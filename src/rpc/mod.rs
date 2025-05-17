use crate::ui::app::App;
use anyhow::{anyhow, Result};
use colored::Colorize;
use ethers::prelude::Middleware;
use ethers::providers::{Http, Provider};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// RPC节点列表
pub const RPC_OPTIONS: [&str; 4] = [
    "https://node1.magnetchain.xyz",
    "https://node2.magnetchain.xyz",
    "https://node3.magnetchain.xyz",
    "https://node4.magnetchain.xyz",
];

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

// 测试所有RPC节点并返回响应时间
pub async fn test_all_rpc_nodes(app_state: &Arc<Mutex<App>>) -> Result<HashMap<&'static str, u64>> {
    let mut response_times = HashMap::new();

    // 并行测试所有RPC节点
    let mut tasks = Vec::new();
    for &rpc in RPC_OPTIONS.iter() {
        let task = tokio::spawn(async move {
            match test_rpc_node(rpc).await {
                Ok(response_time) => Some((rpc, response_time)),
                Err(_) => None,
            }
        });
        tasks.push(task);
    }

    // 等待所有任务完成
    let results = futures::future::join_all(tasks).await;

    // 处理结果
    for result in results {
        if let Ok(Some((rpc, response_time))) = result {
            response_times.insert(rpc, response_time);

            // 更新应用状态中的响应时间
            app_state
                .lock()
                .unwrap()
                .update_rpc_response_time(rpc.to_string(), response_time);

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
pub async fn find_fastest_rpc(app_state: &Arc<Mutex<App>>) -> Result<&'static str> {
    let log_msg = "正在自动测试所有RPC节点以找到最快的节点...";
    println!("{}", log_msg.cyan());
    app_state
        .lock()
        .unwrap()
        .add_log(log_msg.to_string(), crate::ui::app::LogLevel::Info);

    // 测试所有节点
    let response_times = test_all_rpc_nodes(app_state).await?;

    if response_times.is_empty() {
        let error_msg = "所有RPC节点都不可用，请检查网络连接或手动选择节点";
        println!("{}", error_msg.red());
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.to_string(), crate::ui::app::LogLevel::Error);
        return Err(anyhow!(error_msg));
    }

    // 输出所有节点的响应时间
    println!("{}", "RPC节点响应时间:".cyan());
    for (rpc, time) in &response_times {
        println!("  {} - {}ms", rpc, time);
    }

    // 找到响应时间最短的节点
    let fastest_rpc = RPC_OPTIONS
        .iter()
        .filter(|&rpc| response_times.contains_key(rpc))
        .min_by_key(|&rpc| response_times.get(rpc).unwrap_or(&u64::MAX))
        .copied()
        .ok_or_else(|| {
            let error_msg = "无法确定最快的RPC节点，请手动选择节点";
            println!("{}", error_msg.red());
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg.to_string(), crate::ui::app::LogLevel::Error);
            anyhow!(error_msg)
        })?;

    app_state.lock().unwrap().current_rpc = Some(fastest_rpc.to_string());

    let log_msg = format!(
        "选择最快的RPC节点: {} ({}ms)",
        fastest_rpc,
        response_times.get(fastest_rpc).unwrap_or(&0)
    );
    println!("{}", log_msg.green());
    app_state
        .lock()
        .unwrap()
        .add_log(log_msg, crate::ui::app::LogLevel::Success);

    Ok(fastest_rpc)
}

// 手动选择RPC节点
pub async fn select_rpc_node(app_state: &Arc<Mutex<App>>) -> Result<&'static str> {
    // 如果启用了自动选择，则尝试找到最快的节点
    if app_state.lock().unwrap().config.auto_select_rpc {
        match find_fastest_rpc(app_state).await {
            Ok(fastest_rpc) => return Ok(fastest_rpc),
            Err(e) => {
                let error_msg = format!("自动选择RPC节点失败: {}. 将切换到手动选择模式。", e);
                println!("{}", error_msg.yellow());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(error_msg, crate::ui::app::LogLevel::Warning);

                // 自动选择失败后，关闭自动选择功能
                app_state.lock().unwrap().config.auto_select_rpc = false;

                // 继续到手动选择逻辑
            }
        }
    }

    // 手动选择逻辑 - 显示所有可用的RPC节点供选择
    println!("{}", "请选择RPC节点:".cyan());
    for (idx, &rpc) in RPC_OPTIONS.iter().enumerate() {
        println!("  {}. {}", idx + 1, rpc);
    }

    // 使用dialoguer库进行交互式选择
    let selection = dialoguer::Select::new()
        .with_prompt("选择 RPC 节点")
        .default(0)
        .items(&RPC_OPTIONS)
        .interact()
        .map_err(|e| anyhow!("交互选择失败: {}", e))?;

    let selected_rpc = RPC_OPTIONS[selection];

    // 测试所选节点的可用性
    match test_rpc_node(selected_rpc).await {
        Ok(response_time) => {
            app_state
                .lock()
                .unwrap()
                .update_rpc_response_time(selected_rpc.to_string(), response_time);
            app_state.lock().unwrap().current_rpc = Some(selected_rpc.to_string());

            let log_msg = format!("手动选择RPC节点: {} ({}ms)", selected_rpc, response_time);
            println!("{}", log_msg.green());
            app_state
                .lock()
                .unwrap()
                .add_log(log_msg, crate::ui::app::LogLevel::Success);

            Ok(selected_rpc)
        }
        Err(e) => {
            let error_msg = format!("所选RPC节点不可用: {}. 请选择其他节点。", e);
            println!("{}", error_msg.red());
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg, crate::ui::app::LogLevel::Error);

            // 使用 Box::pin 来处理递归调用
            Box::pin(select_rpc_node(app_state)).await
        }
    }
}
