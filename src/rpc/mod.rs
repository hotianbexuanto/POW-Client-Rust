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
        return Err(anyhow!("所有RPC节点都不可用"));
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
        .ok_or_else(|| anyhow!("找不到可用的RPC节点"))?;

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
    // 如果启用了自动选择，则找到最快的节点
    if app_state.lock().unwrap().config.auto_select_rpc {
        find_fastest_rpc(app_state).await
    } else {
        // 否则使用第一个节点（这里可以添加交互式选择逻辑）
        let rpc = RPC_OPTIONS[0];

        // 测试所选节点的可用性
        match test_rpc_node(rpc).await {
            Ok(response_time) => {
                app_state
                    .lock()
                    .unwrap()
                    .update_rpc_response_time(rpc.to_string(), response_time);
                app_state.lock().unwrap().current_rpc = Some(rpc.to_string());

                let log_msg = format!("手动选择RPC节点: {} ({}ms)", rpc, response_time);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(log_msg, crate::ui::app::LogLevel::Info);

                Ok(rpc)
            }
            Err(e) => {
                let error_msg = format!("所选RPC节点不可用: {}", e);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(error_msg, crate::ui::app::LogLevel::Error);

                // 如果手动选择的节点不可用，尝试找到可用的节点
                find_fastest_rpc(app_state).await
            }
        }
    }
}
