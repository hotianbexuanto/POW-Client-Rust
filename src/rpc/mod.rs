use crate::ui::app::App;
use anyhow::{anyhow, Result};
use colored::Colorize;
use ethers::prelude::Middleware;
use ethers::providers::{Http, Provider};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// 导入负载均衡器模块
mod load_balancer;
use load_balancer::{LoadBalancer, NodeStatus};

// RPC节点列表
pub const RPC_OPTIONS: [&str; 4] = [
    "https://node1.magnetchain.xyz",
    "https://node2.magnetchain.xyz",
    "https://node3.magnetchain.xyz",
    "https://node4.magnetchain.xyz",
];

// 全局负载均衡器
lazy_static::lazy_static! {
    static ref LOAD_BALANCER: Mutex<Option<LoadBalancer>> = Mutex::new(None);
}

// 初始化负载均衡器
pub async fn init_load_balancer(app_state: &Arc<Mutex<App>>) -> Result<()> {
    // 将内置节点转换为String
    let default_nodes: Vec<String> = RPC_OPTIONS.iter().map(|&s| s.to_string()).collect();

    // 创建负载均衡器
    let mut balancer = LoadBalancer::new(default_nodes);

    // 加载额外节点
    match load_rpc_nodes_from_file("nodes.txt") {
        Ok(extra_nodes) => {
            if !extra_nodes.is_empty() {
                let log_msg = format!(
                    "从nodes.txt文件加载了{}个额外节点到负载均衡器",
                    extra_nodes.len()
                );
                println!("{}", log_msg.cyan());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(log_msg, crate::ui::app::LogLevel::Info);

                // 将节点添加到负载均衡器
                balancer.add_nodes(extra_nodes);
            }
        }
        Err(e) => {
            let error_msg = format!("读取节点文件失败: {}", e);
            println!("{}", error_msg.yellow());
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg, crate::ui::app::LogLevel::Warning);
        }
    }

    // 初始测试所有节点
    balancer.refresh_node_status(app_state).await?;

    // 更新全局负载均衡器
    *LOAD_BALANCER.lock().unwrap() = Some(balancer);

    // 日志
    let log_msg = "RPC节点负载均衡器已初始化".to_string();
    println!("{}", log_msg.green());
    app_state
        .lock()
        .unwrap()
        .add_log(log_msg, crate::ui::app::LogLevel::Success);

    Ok(())
}

// 获取负载均衡器(如果不存在则创建)
async fn get_load_balancer(app_state: &Arc<Mutex<App>>) -> Result<LoadBalancer> {
    // 检查是否需要初始化
    let needs_init = {
        let lb_guard = LOAD_BALANCER.lock().unwrap();
        lb_guard.is_none()
    };

    // 如果需要初始化，则在锁外初始化
    if needs_init {
        init_load_balancer(app_state).await?;
    }

    // 获取负载均衡器
    let lb_guard = LOAD_BALANCER.lock().unwrap();
    match &*lb_guard {
        Some(lb) => Ok(lb.clone()),
        None => {
            let error_msg = "无法获取RPC节点负载均衡器";
            return Err(anyhow!(error_msg));
        }
    }
}

// 更新App中的节点状态信息
pub fn update_app_node_statuses(app_state: &Arc<Mutex<App>>, balancer: &LoadBalancer) {
    let mut app = app_state.lock().unwrap();

    // 清除现有状态
    app.clear_rpc_node_statuses();

    // 添加节点状态
    for node in balancer.get_all_nodes() {
        let success_rate = if node.success_count + node.fail_count > 0 {
            node.success_count as f64 / (node.success_count + node.fail_count) as f64 * 100.0
        } else {
            0.0
        };

        app.update_rpc_node_status(
            node.url.clone(),
            node.last_response_time,
            node.health_score,
            node.is_active,
            success_rate,
        );
    }
}

// 使用负载均衡器执行请求
pub async fn execute_rpc_request<F, R>(app_state: &Arc<Mutex<App>>, request_func: F) -> Result<R>
where
    F: Fn(&str) -> futures::future::BoxFuture<'static, Result<R>>,
    R: Send + 'static,
{
    // 检查App中是否启用了负载均衡
    let use_load_balancer = {
        let app = app_state.lock().unwrap();
        app.active_load_balancer
    };

    if use_load_balancer {
        // 获取负载均衡器
        let mut balancer = get_load_balancer(app_state).await?;

        // 执行请求
        let result = balancer.smart_request(app_state, request_func).await;

        // 更新全局负载均衡器
        {
            let mut lb_guard = LOAD_BALANCER.lock().unwrap();
            *lb_guard = Some(balancer.clone());
        }

        // 更新App中的节点状态
        update_app_node_statuses(app_state, &balancer);

        result
    } else {
        // 使用当前RPC节点或执行常规选择
        let rpc_url = {
            // 获取当前RPC，如果没有则选择一个新的
            let current_rpc_opt = app_state.lock().unwrap().current_rpc.clone();

            if let Some(url) = current_rpc_opt {
                url
            } else {
                // 不在这里await，而是先释放锁，再选择节点
                select_rpc_node(app_state).await?
            }
        };

        // 直接使用选择的节点执行请求
        request_func(&rpc_url).await
    }
}

// 测试单个RPC节点的响应时间
pub async fn test_rpc_node(rpc_url: &str) -> Result<u64> {
    println!("  正在测试 RPC 节点: {}", rpc_url);

    let provider =
        Provider::<Http>::try_from(rpc_url).map_err(|e| anyhow!("创建RPC提供者失败: {}", e))?;

    let start = Instant::now();
    let _ = provider
        .get_block_number()
        .await
        .map_err(|e| anyhow!("获取区块号失败: {}", e))?;
    let elapsed = start.elapsed();
    let response_time = elapsed.as_millis() as u64;

    println!("  RPC节点 {} 响应时间: {}ms", rpc_url, response_time);

    Ok(response_time)
}

// 从文件加载RPC节点
pub fn load_rpc_nodes_from_file(file_path: &str) -> Result<Vec<String>> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut nodes = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.starts_with("http") {
            nodes.push(trimmed.to_string());
        }
    }

    Ok(nodes)
}

// 测试所有RPC节点并返回响应时间
pub async fn test_all_rpc_nodes(app_state: &Arc<Mutex<App>>) -> Result<HashMap<String, u64>> {
    let mut response_times = HashMap::new();

    // 先加载内置节点
    let mut all_nodes: Vec<String> = RPC_OPTIONS.iter().map(|&s| s.to_string()).collect();

    // 尝试从文件加载额外节点
    match load_rpc_nodes_from_file("nodes.txt") {
        Ok(extra_nodes) => {
            if !extra_nodes.is_empty() {
                let log_msg = format!("从nodes.txt文件加载了{}个额外节点", extra_nodes.len());
                println!("{}", log_msg.cyan());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(log_msg, crate::ui::app::LogLevel::Info);
                all_nodes.extend(extra_nodes);
            }
        }
        Err(e) => {
            let error_msg = format!("读取节点文件失败: {}", e);
            println!("{}", error_msg.yellow());
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg, crate::ui::app::LogLevel::Warning);
        }
    }

    // 并行测试所有RPC节点
    let mut tasks = Vec::new();
    for rpc in all_nodes.clone() {
        let task = tokio::spawn(async move {
            match test_rpc_node(&rpc).await {
                Ok(response_time) => Some((rpc, response_time)),
                Err(_) => None,
            }
        });
        tasks.push(task);
    }

    // 等待所有任务完成
    let results = futures::future::join_all(tasks).await;

    // 处理结果，使用 flatten 优化代码结构
    for result in results.into_iter().flatten() {
        if let Some((rpc, response_time)) = result {
            response_times.insert(rpc.clone(), response_time);

            // 更新应用状态中的响应时间
            app_state
                .lock()
                .unwrap()
                .update_rpc_response_time(rpc.clone(), response_time);

            let log_msg = format!("RPC节点 {} 响应时间: {}ms", rpc, response_time);
            app_state
                .lock()
                .unwrap()
                .add_log(log_msg, crate::ui::app::LogLevel::Info);
        }
    }

    Ok(response_times)
}

// 找到最快的RPC节点
pub async fn find_fastest_rpc(app_state: &Arc<Mutex<App>>) -> Result<String> {
    // 检查是否启用负载均衡
    let use_load_balancer = {
        let app = app_state.lock().unwrap();
        app.active_load_balancer
    };

    if use_load_balancer {
        // 使用负载均衡器选择节点
        let mut balancer = get_load_balancer(app_state).await?;

        // 刷新节点状态
        balancer.refresh_node_status(app_state).await?;

        // 选择最佳节点
        let best_node = balancer
            .select_best_node()
            .ok_or_else(|| anyhow!("无法找到最佳RPC节点"))?;

        // 更新全局负载均衡器
        {
            let mut lb_guard = LOAD_BALANCER.lock().unwrap();
            *lb_guard = Some(balancer.clone());
        }

        // 更新App中的节点状态
        update_app_node_statuses(app_state, &balancer);

        // 更新当前RPC
        {
            let mut app = app_state.lock().unwrap();
            app.current_rpc = Some(best_node.clone());

            let log_msg = format!("负载均衡器选择了最佳节点: {}", best_node);
            println!("{}", log_msg.green());
            app.add_log(log_msg, crate::ui::app::LogLevel::Success);
        }

        return Ok(best_node);
    }

    // 检查自定义RPC节点
    let custom_rpc = {
        let app = app_state.lock().unwrap();
        app.custom_rpc.clone()
    };

    if let Some(custom_rpc) = custom_rpc {
        // 测试自定义节点响应时间
        match test_rpc_node(&custom_rpc).await {
            Ok(response_time) => {
                // 更新应用状态
                {
                    let mut app = app_state.lock().unwrap();
                    app.update_rpc_response_time(custom_rpc.clone(), response_time);
                    app.current_rpc = Some(custom_rpc.clone());

                    let log_msg = format!(
                        "优先使用自定义RPC节点: {} ({}ms)",
                        custom_rpc, response_time
                    );
                    println!("{}", log_msg.green());
                    app.add_log(log_msg, crate::ui::app::LogLevel::Success);
                }

                return Ok(custom_rpc);
            }
            Err(e) => {
                let error_msg = format!("自定义RPC节点不可用: {}. 将尝试其他节点。", e);
                println!("{}", error_msg.yellow());

                {
                    let mut app = app_state.lock().unwrap();
                    app.add_log(error_msg, crate::ui::app::LogLevel::Warning);
                    // 不要清除自定义RPC，只是这次不使用它
                }
            }
        }
    }

    let log_msg = "正在自动测试所有RPC节点以找到最快的节点...";
    println!("{}", log_msg.cyan());
    {
        let mut app = app_state.lock().unwrap();
        app.add_log(log_msg.to_string(), crate::ui::app::LogLevel::Info);
    }

    // 测试所有节点
    let response_times = test_all_rpc_nodes(app_state).await?;

    if response_times.is_empty() {
        let error_msg = "所有RPC节点都不可用，请检查网络连接或手动选择节点";
        println!("{}", error_msg.red());
        {
            let mut app = app_state.lock().unwrap();
            app.add_log(error_msg.to_string(), crate::ui::app::LogLevel::Error);
        }
        return Err(anyhow!(error_msg));
    }

    // 输出所有节点的响应时间
    println!("{}", "RPC节点响应时间:".cyan());
    for (rpc, time) in &response_times {
        println!("  {} - {}ms", rpc, time);
    }

    // 找到响应时间最短的节点
    let fastest_rpc = response_times
        .iter()
        .min_by_key(|&(_, time)| time)
        .map(|(rpc, _)| rpc.clone())
        .ok_or_else(|| {
            let error_msg = "无法确定最快的RPC节点，请手动选择节点";
            println!("{}", error_msg.red());

            let mut app = app_state.lock().unwrap();
            app.add_log(error_msg.to_string(), crate::ui::app::LogLevel::Error);

            anyhow!(error_msg)
        })?;

    {
        let mut app = app_state.lock().unwrap();
        app.current_rpc = Some(fastest_rpc.clone());

        let log_msg = format!(
            "选择最快的RPC节点: {} ({}ms)",
            fastest_rpc,
            response_times.get(&fastest_rpc).unwrap_or(&0)
        );
        println!("{}", log_msg.green());
        app.add_log(log_msg, crate::ui::app::LogLevel::Success);
    }

    Ok(fastest_rpc)
}

// 手动选择RPC节点
pub async fn select_rpc_node(app_state: &Arc<Mutex<App>>) -> Result<String> {
    // 检查是否启用了负载均衡，使用本地变量存储状态
    let load_balancer_active = app_state.lock().unwrap().active_load_balancer;

    if load_balancer_active {
        return find_fastest_rpc(app_state).await;
    }

    // 首先检查是否有自定义RPC节点
    let custom_rpc = {
        let app = app_state.lock().unwrap();
        app.custom_rpc.clone()
    };

    if let Some(custom_rpc) = custom_rpc {
        // 测试自定义RPC节点是否可用
        match test_rpc_node(&custom_rpc).await {
            Ok(response_time) => {
                {
                    let mut app = app_state.lock().unwrap();
                    app.update_rpc_response_time(custom_rpc.clone(), response_time);
                    app.current_rpc = Some(custom_rpc.clone());

                    let log_msg =
                        format!("使用自定义RPC节点: {} ({}ms)", custom_rpc, response_time);
                    println!("{}", log_msg.green());
                    app.add_log(log_msg, crate::ui::app::LogLevel::Success);
                }

                return Ok(custom_rpc);
            }
            Err(e) => {
                let error_msg = format!("自定义RPC节点不可用: {}. 将尝试其他节点。", e);
                println!("{}", error_msg.yellow());
                {
                    let mut app = app_state.lock().unwrap();
                    app.add_log(error_msg, crate::ui::app::LogLevel::Warning);
                }
                // 继续下一步自动或手动选择
            }
        }
    }

    // 加载额外的RPC节点
    let mut all_nodes: Vec<String> = RPC_OPTIONS.iter().map(|&s| s.to_string()).collect();
    match load_rpc_nodes_from_file("nodes.txt") {
        Ok(extra_nodes) => {
            if !extra_nodes.is_empty() {
                let log_msg = format!("从nodes.txt文件加载了{}个额外节点", extra_nodes.len());
                println!("{}", log_msg.cyan());
                {
                    let mut app = app_state.lock().unwrap();
                    app.add_log(log_msg, crate::ui::app::LogLevel::Info);
                }
                all_nodes.extend(extra_nodes);
            }
        }
        Err(e) => {
            let log_msg = format!("读取节点文件时出错: {}", e);
            println!("{}", log_msg.yellow());
            {
                let mut app = app_state.lock().unwrap();
                app.add_log(log_msg, crate::ui::app::LogLevel::Warning);
            }
        }
    }

    // 如果用户再次启用了自定义RPC（可能在处理过程中被添加）
    let custom_rpc = {
        let app = app_state.lock().unwrap();
        app.custom_rpc.clone()
    };

    if let Some(custom_rpc) = custom_rpc {
        // 确保自定义RPC不在常规节点列表中
        if !all_nodes.contains(&custom_rpc) {
            all_nodes.insert(0, custom_rpc.clone()); // 添加到列表开头，确保它出现在选择界面顶部
        }
    }

    // 如果启用了自动选择，则尝试找到最快的节点
    let auto_select = {
        let app = app_state.lock().unwrap();
        app.config.auto_select_rpc
    };

    if auto_select {
        match find_fastest_rpc(app_state).await {
            Ok(fastest_rpc) => return Ok(fastest_rpc),
            Err(e) => {
                let error_msg = format!("自动选择RPC节点失败: {}. 将切换到手动选择模式。", e);
                println!("{}", error_msg.yellow());
                {
                    let mut app = app_state.lock().unwrap();
                    app.add_log(error_msg, crate::ui::app::LogLevel::Warning);

                    // 自动选择失败后，关闭自动选择功能
                    app.config.auto_select_rpc = false;
                }
                // 继续到手动选择逻辑
            }
        }
    }

    // 手动选择逻辑 - 显示所有可用的RPC节点供选择
    println!("{}", "请选择RPC节点:".cyan());
    for (idx, rpc) in all_nodes.iter().enumerate() {
        println!("  {}. {}", idx + 1, rpc);
    }

    // 使用dialoguer库进行交互式选择
    let selection = dialoguer::Select::new()
        .with_prompt("选择 RPC 节点")
        .default(0)
        .items(&all_nodes)
        .interact()
        .map_err(|e| anyhow!("交互选择失败: {}", e))?;

    let selected_rpc = &all_nodes[selection];

    // 测试所选节点的可用性
    match test_rpc_node(selected_rpc).await {
        Ok(response_time) => {
            {
                let mut app = app_state.lock().unwrap();
                app.update_rpc_response_time(selected_rpc.to_string(), response_time);
                app.current_rpc = Some(selected_rpc.to_string());

                let log_msg = format!("手动选择RPC节点: {} ({}ms)", selected_rpc, response_time);
                println!("{}", log_msg.green());
                app.add_log(log_msg, crate::ui::app::LogLevel::Success);
            }

            Ok(selected_rpc.to_string())
        }
        Err(e) => {
            let error_msg = format!("所选RPC节点不可用: {}. 请选择其他节点。", e);
            println!("{}", error_msg.red());
            {
                let mut app = app_state.lock().unwrap();
                app.add_log(error_msg, crate::ui::app::LogLevel::Error);
            }

            // 使用 Box::pin 来处理递归调用
            Box::pin(select_rpc_node(app_state)).await
        }
    }
}
