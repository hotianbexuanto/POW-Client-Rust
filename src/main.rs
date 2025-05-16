use anyhow::{anyhow, Result};
use colored::*;
use dialoguer::{Input, Select};
use ethers::{
    abi::Token,
    prelude::*,
    providers::{Http, Provider},
    utils::keccak256,
};
use num_bigint::BigUint;
use ratatui::backend::CrosstermBackend;
use std::{
    convert::TryFrom,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;

mod contract;
mod ui;

use contract::MiningContract;
use ui::{create_app, destroy_tui, init_tui, render, App, AppState, Event, LogLevel};

// 定义常量
const CONTRACT_ADDRESS: &str = "0x51e0ab7f7db4a2bf4500dfa59f7a4957afc8c02e";
const RPC_OPTIONS: [&str; 4] = [
    "https://node1.magnetchain.xyz",
    "https://node2.magnetchain.xyz",
    "https://node3.magnetchain.xyz",
    "https://node4.magnetchain.xyz",
];
const MIN_WALLET_BALANCE: f64 = 0.1;
const MIN_CONTRACT_BALANCE: f64 = 3.0;
const MAX_RETRIES: usize = 5;
const MINING_TIMEOUT_SECS: u64 = 600; // 10分钟
                                      // 并行任务数
const PARALLEL_TASKS: usize = 3; // 同时处理的任务数量
                                 // MagnetChain的chainId
const CHAIN_ID: u64 = 114514; // 修正为正确的链ID

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化应用状态
    let app_state = create_app();

    print_welcome_message();

    // 选择RPC节点
    let rpc_url = select_rpc_node()?;
    println!(
        "{}",
        format!("已选择 RPC / Selected RPC: {}", rpc_url).green()
    );

    // 添加到日志
    app_state
        .lock()
        .unwrap()
        .add_log(format!("已选择 RPC: {}", rpc_url), LogLevel::Info);

    // 初始化以太坊提供者
    let provider = Provider::<Http>::try_from(rpc_url)?;

    // 显示链ID信息
    match provider.get_chainid().await {
        Ok(chainid) => {
            let log_msg = format!("连接到链ID: {}", chainid);
            println!("{}", log_msg.green());

            app_state.lock().unwrap().add_log(log_msg, LogLevel::Info);

            if chainid != U256::from(CHAIN_ID) {
                let warning_msg = format!("警告：检测到的链ID与设置的不符！");
                println!("{}", warning_msg.yellow());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(warning_msg, LogLevel::Warning);
            }
        }
        Err(e) => {
            let error_msg = format!("无法获取链ID: {}", e);
            println!("{}", error_msg.yellow());
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg, LogLevel::Error);
        }
    }

    // 输入私钥并创建钱包
    let wallet = input_private_key(provider, &app_state).await?;
    let wallet_address = wallet.address();

    let wallet_msg = format!("钱包地址: {}", wallet_address);
    println!("{}", wallet_msg.green());
    app_state
        .lock()
        .unwrap()
        .add_log(wallet_msg, LogLevel::Info);

    // 检查钱包余额
    let balance = check_wallet_balance(&wallet, &app_state).await?;
    let balance_eth = ethers::utils::format_ether(balance);
    let balance_f64 = balance_eth.parse::<f64>().unwrap_or(0.0);

    // 更新应用状态中的钱包信息
    app_state
        .lock()
        .unwrap()
        .update_wallet_info(wallet_address, balance_f64);

    // 初始化合约
    let contract = init_contract(wallet, &app_state).await?;

    // 检查合约余额
    let contract_balance = check_contract_balance(&contract, &app_state).await?;
    let contract_balance_eth = ethers::utils::format_ether(contract_balance);
    let contract_balance_f64 = contract_balance_eth.parse::<f64>().unwrap_or(0.0);

    // 更新应用状态中的合约信息
    let contract_address = contract.address();
    app_state
        .lock()
        .unwrap()
        .update_contract_info(contract_address, contract_balance_f64);

    // 开始挖矿循环
    let mining_msg = "开始挖矿 - 免费挖矿 (3 MAG 每次哈希)";
    println!("{}", mining_msg.bold().green());
    app_state
        .lock()
        .unwrap()
        .add_log(mining_msg.to_string(), LogLevel::Success);

    // 初始化TUI
    let (mut terminal, _tx, mut rx) = init_tui()?;

    // 在单独的线程中运行挖矿
    let mining_app_state = app_state.clone();
    tokio::spawn(async move {
        if let Err(e) = start_mining_loop(contract, mining_app_state.clone()).await {
            let error_msg = format!("挖矿出错: {}", e);
            println!("{}", error_msg.red());
            mining_app_state
                .lock()
                .unwrap()
                .add_log(error_msg, LogLevel::Error);
        }
    });

    // 启动UI循环
    loop {
        // 渲染TUI
        terminal.draw(|f| {
            render::<CrosstermBackend<std::io::Stdout>>(f, &app_state.lock().unwrap());
        })?;

        // 处理事件
        match rx.recv().await {
            Some(Event::Input(_)) => {
                // 用户按下q或Ctrl+C等退出键
                app_state.lock().unwrap().state = AppState::Exiting;
                break;
            }
            Some(Event::Tick) => {
                // 定时更新 - 可以在这里更新一些实时数据
            }
            None => break,
        }

        // 检查是否应该退出
        if app_state.lock().unwrap().state == AppState::Exiting {
            break;
        }
    }

    // 清理并退出
    destroy_tui(&mut terminal)?;

    println!("程序已退出 / Program exited.");
    Ok(())
}

fn print_welcome_message() {
    println!(
        "{}",
        " 你好，欢迎使用 Magnet POW 区块链挖矿客户端！ "
            .bold()
            .on_cyan()
            .black()
    );
    println!(
        "{}",
        " Hello, welcome to Magnet POW Blockchain Mining Client! "
            .bold()
            .on_cyan()
            .black()
    );
    println!(
        "{}",
        "启动挖矿客户端，需要确保钱包里有0.1MAG，如果没有，加入TG群免费领取0.1 MAG空投。"
            .bold()
            .magenta()
    );
    println!("{}", "To start the mining client, ensure your wallet has 0.1 MAG. If not, join the Telegram group for a free 0.1 MAG airdrop.".bold().magenta());
    println!(
        "{}",
        "TG群链接 / Telegram group link: https://t.me/MagnetPOW"
            .bold()
            .magenta()
    );
    println!(
        "{}",
        format!(
            "网络信息 / Network Info: 链ID / Chain ID: {}, 货币符号 / Symbol: MAG",
            CHAIN_ID
        )
        .cyan()
    );
}

fn select_rpc_node() -> Result<&'static str> {
    println!("{}", "\n选择 RPC 节点 / Select RPC Node:".bold());

    for (i, rpc) in RPC_OPTIONS.iter().enumerate() {
        println!("{}", format!("{}. {}", i + 1, rpc).cyan());
    }

    let selection = Select::new()
        .with_prompt("选择节点 / Select node")
        .items(&RPC_OPTIONS)
        .default(0)
        .interact()?;

    Ok(RPC_OPTIONS[selection])
}

async fn input_private_key<P: JsonRpcClient + 'static + Clone>(
    provider: Provider<P>,
    app_state: &Arc<Mutex<App>>,
) -> Result<SignerMiddleware<Provider<P>, LocalWallet>> {
    let max_attempts = 3;
    let mut attempts = 0;

    while attempts < max_attempts {
        let private_key: String = Input::new()
            .with_prompt("\n请输入私钥 / Enter private key (starts with 0x)")
            .validate_with(|input: &String| -> Result<(), &str> {
                if input.starts_with("0x") && input.len() == 66 && hex::decode(&input[2..]).is_ok() {
                    Ok(())
                } else {
                    Err("私钥格式错误：需以0x开头，后面跟64位十六进制字符 / Invalid private key: Must start with 0x followed by 64 hexadecimal characters")
                }
            })
            .interact()?;

        match private_key.parse::<LocalWallet>() {
            Ok(mut wallet) => {
                // 设置钱包的chainId
                wallet = wallet.with_chain_id(CHAIN_ID);
                let msg = format!("已设置钱包chainId为: {}", CHAIN_ID);
                println!("{}", msg.green());
                app_state.lock().unwrap().add_log(msg, LogLevel::Info);

                let client = SignerMiddleware::new(provider.clone(), wallet);
                return Ok(client);
            }
            Err(e) => {
                attempts += 1;
                let error_msg = format!(
                    "私钥解析错误: {}. 还剩 {} 次尝试。",
                    e,
                    max_attempts - attempts
                );
                eprintln!("{}", error_msg.red());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(error_msg, LogLevel::Error);

                if attempts == max_attempts {
                    return Err(anyhow!("达到最大尝试次数，程序退出"));
                }
            }
        }
    }

    Err(anyhow!("无法解析私钥"))
}

async fn check_wallet_balance<M: Middleware + 'static>(
    wallet: &SignerMiddleware<M, LocalWallet>,
    app_state: &Arc<Mutex<App>>,
) -> Result<U256> {
    let balance = wallet.get_balance(wallet.address(), None).await?;
    let balance_msg = format!("当前余额: {} MAG", ethers::utils::format_ether(balance));
    println!("{}", balance_msg.green());
    app_state
        .lock()
        .unwrap()
        .add_log(balance_msg, LogLevel::Info);

    let min_balance = ethers::utils::parse_ether(MIN_WALLET_BALANCE)?;
    if balance < min_balance {
        let error_msg = format!(
            "钱包余额不足: {} MAG (需要至少 {} MAG)\n请通过 Telegram 群领取免费 MAG 或充值。",
            ethers::utils::format_ether(balance),
            MIN_WALLET_BALANCE
        );
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.clone(), LogLevel::Error);
        return Err(anyhow!(error_msg));
    }

    Ok(balance)
}

async fn init_contract<M: Middleware + 'static>(
    wallet: SignerMiddleware<M, LocalWallet>,
    app_state: &Arc<Mutex<App>>,
) -> Result<MiningContract<SignerMiddleware<M, LocalWallet>>> {
    let contract_address = CONTRACT_ADDRESS.parse::<Address>()?;

    // 显示当前钱包信息和设置
    let info_msg = format!(
        "钱包地址: {}, 链ID: {}, 合约地址: {}",
        wallet.address(),
        CHAIN_ID,
        contract_address
    );
    println!("{}", info_msg.cyan());
    app_state.lock().unwrap().add_log(info_msg, LogLevel::Info);

    let contract = MiningContract::new(contract_address, Arc::new(wallet));
    Ok(contract)
}

async fn check_contract_balance<M: Middleware + 'static>(
    contract: &MiningContract<M>,
    app_state: &Arc<Mutex<App>>,
) -> Result<U256> {
    let contract_balance = contract.get_contract_balance().call().await?;
    let balance_msg = format!(
        "池中余额: {} MAG",
        ethers::utils::format_ether(contract_balance)
    );
    println!("{}", balance_msg.green());
    app_state
        .lock()
        .unwrap()
        .add_log(balance_msg, LogLevel::Info);

    let min_contract_balance = ethers::utils::parse_ether(MIN_CONTRACT_BALANCE)?;
    if contract_balance < min_contract_balance {
        let error_msg = format!(
            "合约余额不足: {} MAG (需要至少 {} MAG)\n请联系 Magnet 链管理员充值合约。",
            ethers::utils::format_ether(contract_balance),
            MIN_CONTRACT_BALANCE
        );
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.clone(), LogLevel::Error);
        return Err(anyhow!(error_msg));
    }

    Ok(contract_balance)
}

async fn start_mining_loop<M: Middleware + 'static>(
    contract: MiningContract<SignerMiddleware<M, LocalWallet>>,
    app_state: Arc<Mutex<App>>,
) -> Result<()> {
    // 未使用的变量标记为_开头或移除
    let _retry_count = 0;
    let _rpc_index = 0;

    // 创建共享的任务状态
    let active_tasks = Arc::new(AtomicUsize::new(0));

    // 更新应用状态中的任务数量
    app_state.lock().unwrap().mining_status.total_tasks = PARALLEL_TASKS;

    // 启动多个并行任务处理器
    let mut task_handles = Vec::new();
    for task_id in 0..PARALLEL_TASKS {
        let contract_clone = contract.clone();
        let active_tasks_clone = active_tasks.clone();
        let task_app_state = app_state.clone();

        // 在UI中初始化任务
        app_state
            .lock()
            .unwrap()
            .update_task(task_id, "准备中".to_string());

        let handle = tokio::spawn(async move {
            let mut local_retry_count = 0;

            loop {
                active_tasks_clone.fetch_add(1, Ordering::SeqCst);
                let task_msg = format!("任务 #{}: 开始处理新挖矿任务", task_id);
                task_app_state
                    .lock()
                    .unwrap()
                    .update_task(task_id, "开始中".to_string());
                task_app_state
                    .lock()
                    .unwrap()
                    .add_log(task_msg, LogLevel::Info);

                match mine_once(&contract_clone, task_id, &task_app_state).await {
                    Ok(_) => {
                        local_retry_count = 0; // 重置重试计数
                        let success_msg = format!("任务 #{}: 成功完成", task_id);
                        task_app_state
                            .lock()
                            .unwrap()
                            .update_task(task_id, "成功".to_string());
                        task_app_state
                            .lock()
                            .unwrap()
                            .add_log(success_msg, LogLevel::Success);
                        task_app_state.lock().unwrap().add_solution_found();
                    }
                    Err(err) => {
                        let err_str = format!("{:?}", err);
                        let error_msg = format!("任务 #{}: 出错: {}", task_id, err_str);
                        task_app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);

                        if err_str.contains("network")
                            || err_str.contains("timeout")
                            || err_str.contains("connection")
                        {
                            // 网络错误处理
                            let net_error_msg = format!("任务 #{}: 网络错误", task_id);
                            task_app_state
                                .lock()
                                .unwrap()
                                .update_task(task_id, "网络错误".to_string());
                            task_app_state
                                .lock()
                                .unwrap()
                                .add_log(net_error_msg, LogLevel::Warning);
                            local_retry_count += 1;
                        } else {
                            // 其他错误
                            task_app_state
                                .lock()
                                .unwrap()
                                .update_task(task_id, "错误".to_string());
                            local_retry_count += 1;
                        }

                        if local_retry_count >= MAX_RETRIES {
                            let max_retry_msg =
                                format!("任务 #{}: 达到最大重试次数，任务终止", task_id);
                            task_app_state
                                .lock()
                                .unwrap()
                                .update_task(task_id, "终止".to_string());
                            task_app_state
                                .lock()
                                .unwrap()
                                .add_log(max_retry_msg, LogLevel::Error);
                            break;
                        }

                        let retry_msg = format!(
                            "任务 #{}: 5秒后重试 (第{}/{})",
                            task_id, local_retry_count, MAX_RETRIES
                        );
                        task_app_state.lock().unwrap().update_task(
                            task_id,
                            format!("重试 {}/{}", local_retry_count, MAX_RETRIES),
                        );
                        task_app_state
                            .lock()
                            .unwrap()
                            .add_log(retry_msg, LogLevel::Warning);
                        sleep(Duration::from_secs(5)).await;
                    }
                }
                active_tasks_clone.fetch_sub(1, Ordering::SeqCst);

                // 短暂延迟，避免连续请求
                sleep(Duration::from_secs(2)).await;
            }
        });

        task_handles.push(handle);
    }

    // 监控任务状态
    let monitor_app_state = app_state.clone();
    tokio::spawn(async move {
        loop {
            let current_active = active_tasks.load(Ordering::SeqCst);
            let status_msg = format!("当前活跃任务数: {}", current_active);

            // 复制总哈希率
            let total_hash_rate = {
                let app = monitor_app_state.lock().unwrap();
                app.mining_status.total_hash_rate
            };

            // 更新UI中的活跃任务数
            {
                let mut app = monitor_app_state.lock().unwrap();
                app.update_mining_status(current_active, total_hash_rate);
                app.add_log(status_msg, LogLevel::Info);
            }

            sleep(Duration::from_secs(30)).await;
        }
    });

    // 等待所有任务完成（实际上不会完成，除非出错）
    for handle in task_handles {
        if let Err(e) = handle.await {
            let error_msg = format!("任务出错: {}", e);
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg, LogLevel::Error);
        }
    }

    // 所有任务都结束时，返回错误
    app_state
        .lock()
        .unwrap()
        .add_log("所有挖矿任务都已终止".to_string(), LogLevel::Error);
    Err(anyhow!("所有挖矿任务都已终止"))
}

async fn mine_once<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    task_id: usize,
    app_state: &Arc<Mutex<App>>,
) -> Result<()> {
    // 请求新任务
    let task_msg = format!("任务 #{}: 请求新挖矿任务...", task_id);
    app_state.lock().unwrap().add_log(task_msg, LogLevel::Info);

    // 获取当前gas价格
    let gas_price = match contract.client().get_gas_price().await {
        Ok(price) => {
            let msg = format!(
                "任务 #{}: 获取到当前gas价格: {} gwei",
                task_id,
                ethers::utils::format_units(price, "gwei")?
            );
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);
            price
        }
        Err(e) => {
            let msg = format!("任务 #{}: 获取gas价格失败，使用默认值: {}", task_id, e);
            app_state.lock().unwrap().add_log(msg, LogLevel::Warning);
            U256::from(25_000_000_001u64) // 25 gwei 默认值
        }
    };

    // 估算gas限制
    let gas_limit = match contract.request_mining_task().estimate_gas().await {
        Ok(limit) => {
            // 增加10%余量 (limit * 110 / 100)
            let adjusted_limit = limit.saturating_mul(U256::from(110)) / U256::from(100);
            let msg = format!(
                "任务 #{}: 估算gas限制: {}, 调整后: {}",
                task_id, limit, adjusted_limit
            );
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);
            adjusted_limit
        }
        Err(e) => {
            let msg = format!("任务 #{}: 估算gas限制失败，使用默认值: {}", task_id, e);
            app_state.lock().unwrap().add_log(msg, LogLevel::Warning);
            U256::from(300_000u64) // 使用默认值
        }
    };

    // 打印交易发送详情
    let tx_msg = format!(
        "任务 #{}: 准备发送交易：gas限制={}, gas价格={} gwei, chainId={}",
        task_id,
        gas_limit,
        ethers::utils::format_units(gas_price, "gwei")?,
        CHAIN_ID
    );
    app_state.lock().unwrap().add_log(tx_msg, LogLevel::Info);

    // 发送交易 - 使用多个let绑定来避免临时值被释放
    let task = contract.request_mining_task();
    let task_with_gas = task.gas(gas_limit);
    let task_with_gas_price = task_with_gas.gas_price(gas_price);
    let tx_result = task_with_gas_price.send().await;

    let tx = match tx_result {
        Ok(pending_tx) => {
            let msg = format!("任务 #{}: 交易已发送，等待确认...", task_id);
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);
            app_state
                .lock()
                .unwrap()
                .update_task(task_id, "等待确认".to_string());

            match pending_tx.await {
                Ok(Some(receipt)) => receipt,
                Ok(None) => {
                    let error_msg = format!("任务 #{}: 交易没有收据", task_id);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg.clone(), LogLevel::Error);
                    return Err(anyhow!(error_msg));
                }
                Err(e) => {
                    let err_msg = format!("任务 #{}: 交易确认失败: {:?}", task_id, e);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(err_msg.clone(), LogLevel::Error);
                    return Err(anyhow!(err_msg));
                }
            }
        }
        Err(e) => {
            let err_msg = format!("任务 #{}: 交易发送失败: {:?}", task_id, e);
            app_state
                .lock()
                .unwrap()
                .add_log(err_msg.clone(), LogLevel::Error);
            return Err(anyhow!(err_msg));
        }
    };

    let success_msg = format!(
        "任务 #{}: 任务请求成功, 交易哈希: {}",
        task_id, tx.transaction_hash
    );
    app_state
        .lock()
        .unwrap()
        .add_log(success_msg, LogLevel::Success);

    // 获取任务
    let task = contract.get_my_task().call().await?;

    if !task.2 {
        // 如果任务不活跃
        let error_msg = format!("任务 #{}: 没有活跃的挖矿任务", task_id);
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.clone(), LogLevel::Warning);
        app_state
            .lock()
            .unwrap()
            .update_task(task_id, "无任务".to_string());
        sleep(Duration::from_secs(5)).await;
        return Err(anyhow!(error_msg));
    }

    let nonce = task.0;
    let difficulty = task.1;

    // 更新UI中的任务数据
    app_state
        .lock()
        .unwrap()
        .update_task_data(task_id, nonce, difficulty);

    let task_info_msg = format!(
        "任务 #{}: 任务: nonce={}, difficulty={}",
        task_id, nonce, difficulty
    );
    app_state
        .lock()
        .unwrap()
        .add_log(task_info_msg, LogLevel::Info);
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "计算中".to_string());

    // 获取钱包地址（从合约实例的签名者中提取）
    let wallet_address = contract.client().address();

    // 计算解决方案
    let calc_msg = format!("任务 #{}: 正在计算解决方案...", task_id);
    app_state.lock().unwrap().add_log(calc_msg, LogLevel::Info);

    let solution = await_solution(
        nonce,
        wallet_address,
        difficulty,
        task_id,
        app_state.clone(),
    )
    .await?;

    let solution_msg = format!("任务 #{}: 找到解决方案: {}", task_id, solution);
    app_state
        .lock()
        .unwrap()
        .add_log(solution_msg, LogLevel::Success);
    app_state
        .lock()
        .unwrap()
        .update_task_solution(task_id, solution);

    // 验证任务是否仍然有效
    let current_task = contract.get_my_task().call().await?;
    if !current_task.2 || current_task.0 != nonce {
        let error_msg = format!("任务 #{}: 任务已失效，重新请求", task_id);
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.clone(), LogLevel::Warning);
        app_state
            .lock()
            .unwrap()
            .update_task(task_id, "已失效".to_string());
        return Err(anyhow!(error_msg));
    }

    // 检查合约余额
    let contract_balance = contract.get_contract_balance().call().await?;
    let min_contract_balance = ethers::utils::parse_ether(MIN_CONTRACT_BALANCE)?;
    if contract_balance < min_contract_balance {
        let error_msg = format!("任务 #{}: 合约余额不足，无法提交", task_id);
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.clone(), LogLevel::Error);
        app_state
            .lock()
            .unwrap()
            .update_task(task_id, "余额不足".to_string());
        return Err(anyhow!(error_msg));
    }

    // 提交解决方案
    let submit_msg = format!("任务 #{}: 提交解决方案...", task_id);
    app_state
        .lock()
        .unwrap()
        .add_log(submit_msg, LogLevel::Info);
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "提交中".to_string());

    // 获取当前gas价格（提交时再次更新）
    let gas_price = match contract.client().get_gas_price().await {
        Ok(price) => {
            let msg = format!(
                "任务 #{}: 获取到当前gas价格: {} gwei",
                task_id,
                ethers::utils::format_units(price, "gwei")?
            );
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);
            price
        }
        Err(e) => {
            let msg = format!("任务 #{}: 获取gas价格失败，使用默认值: {}", task_id, e);
            app_state.lock().unwrap().add_log(msg, LogLevel::Warning);
            U256::from(25_000_000_001u64) // 25 gwei 默认值
        }
    };

    // 估算提交解决方案的gas限制
    let submit_gas_limit = match contract.submit_mining_result(solution).estimate_gas().await {
        Ok(limit) => {
            // 增加10%余量 (limit * 110 / 100)
            let adjusted_limit = limit.saturating_mul(U256::from(110)) / U256::from(100);
            let msg = format!(
                "任务 #{}: 估算提交gas限制: {}, 调整后: {}",
                task_id, limit, adjusted_limit
            );
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);
            adjusted_limit
        }
        Err(e) => {
            let msg = format!("任务 #{}: 估算提交gas限制失败，使用默认值: {}", task_id, e);
            app_state.lock().unwrap().add_log(msg, LogLevel::Warning);
            U256::from(300_000u64) // 使用默认值
        }
    };

    // 发送提交交易 - 使用多个let绑定来避免临时值被释放
    let submit_task = contract.submit_mining_result(solution);
    let submit_task_with_gas = submit_task.gas(submit_gas_limit);
    let submit_task_with_gas_price = submit_task_with_gas.gas_price(gas_price);
    let submit_result = submit_task_with_gas_price.send().await;

    let submit_tx = match submit_result {
        Ok(pending_tx) => {
            let msg = format!("任务 #{}: 提交交易已发送，等待确认...", task_id);
            app_state.lock().unwrap().add_log(msg, LogLevel::Info);

            match pending_tx.await {
                Ok(Some(receipt)) => receipt,
                Ok(None) => {
                    let error_msg = format!("任务 #{}: 提交交易没有收据", task_id);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg.clone(), LogLevel::Error);
                    app_state
                        .lock()
                        .unwrap()
                        .update_task(task_id, "提交失败".to_string());
                    return Err(anyhow!(error_msg));
                }
                Err(e) => {
                    let err_msg = format!("任务 #{}: 提交交易确认失败: {:?}", task_id, e);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(err_msg.clone(), LogLevel::Error);
                    app_state
                        .lock()
                        .unwrap()
                        .update_task(task_id, "确认失败".to_string());
                    return Err(anyhow!(err_msg));
                }
            }
        }
        Err(e) => {
            let err_msg = format!("任务 #{}: 提交交易发送失败: {:?}", task_id, e);
            app_state
                .lock()
                .unwrap()
                .add_log(err_msg.clone(), LogLevel::Error);
            app_state
                .lock()
                .unwrap()
                .update_task(task_id, "发送失败".to_string());
            return Err(anyhow!(err_msg));
        }
    };

    let success_submit_msg = format!(
        "任务 #{}: 提交成功, 交易哈希: {}",
        task_id, submit_tx.transaction_hash
    );
    app_state
        .lock()
        .unwrap()
        .add_log(success_submit_msg, LogLevel::Success);
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "成功".to_string());

    // 显示余额变化
    let new_balance = contract
        .client()
        .get_balance(contract.client().address(), None)
        .await?;
    let balance_msg = format!(
        "任务 #{}: 当前余额: {} MAG",
        task_id,
        ethers::utils::format_ether(new_balance)
    );
    app_state
        .lock()
        .unwrap()
        .add_log(balance_msg, LogLevel::Info);

    // 更新钱包余额
    let balance_eth = ethers::utils::format_ether(new_balance);
    let balance_f64 = balance_eth.parse::<f64>().unwrap_or(0.0);
    app_state
        .lock()
        .unwrap()
        .update_wallet_info(wallet_address, balance_f64);

    Ok(())
}

// 添加await_solution函数，用于等待挖矿解决方案
async fn await_solution(
    nonce: U256,
    address: Address,
    difficulty: U256,
    task_id: usize,
    app_state: Arc<Mutex<App>>,
) -> Result<U256> {
    // 设置超时等待挖矿解决方案
    match tokio::time::timeout(
        Duration::from_secs(MINING_TIMEOUT_SECS),
        mine_solution(nonce, address, difficulty, task_id, app_state.clone()),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let timeout_msg = format!("任务 #{}: 挖矿超时，停止尝试", task_id);
            app_state
                .lock()
                .unwrap()
                .add_log(timeout_msg, LogLevel::Error);
            app_state
                .lock()
                .unwrap()
                .update_task(task_id, "超时".to_string());
            Err(anyhow!("任务 #{}: 挖矿超时", task_id))
        }
    }
}

// 恢复并修改mine_solution函数
async fn mine_solution(
    nonce: U256,
    address: Address,
    difficulty: U256,
    task_id: usize,
    app_state: Arc<Mutex<App>>,
) -> Result<U256> {
    // 优化1: 增加线程数量，默认CPU核心数，但最少4个线程
    let num_threads = std::cmp::max(num_cpus::get(), 4);
    let solution_found = Arc::new(AtomicBool::new(false));
    let found_solution = Arc::new(AtomicU64::new(0));
    let total_hashes = Arc::new(AtomicU64::new(0));

    // 优化2: 记录上次找到解决方案的统计信息，用于启发式搜索
    static LAST_SUCCESSFUL_THREAD: AtomicUsize = AtomicUsize::new(0);
    static LAST_SOLUTION_RANGE: AtomicU64 = AtomicU64::new(0);

    let start_time = Instant::now();

    // 在TUI模式下，不使用进度条显示，而是更新应用状态
    let mining_msg = format!(
        "任务 #{}: 开始挖矿计算，使用 {} 个线程",
        task_id, num_threads
    );
    app_state
        .lock()
        .unwrap()
        .add_log(mining_msg, LogLevel::Info);

    // 优化3: 使用更精确的阈值计算
    let threshold = {
        let max_u256 = BigUint::from(2u32).pow(256) - BigUint::from(1u32);
        let diff = BigUint::from(difficulty.as_u128());
        max_u256 / diff
    };

    // 预计算编码前缀
    let prefix = solidity_pack_uint_address(nonce, address)?;

    // 创建多个挖矿任务，考虑之前的成功记录
    let mut handles = vec![];
    let last_successful_thread = LAST_SUCCESSFUL_THREAD.load(Ordering::Relaxed);
    let last_solution_range = LAST_SOLUTION_RANGE.load(Ordering::Relaxed);

    // 预分配缓冲区，提高性能
    let buffer_size = 32 * 1024; // 32KB预分配缓冲区

    for thread_id in 0..num_threads {
        let prefix = prefix.clone();
        let solution_found = solution_found.clone();
        let found_solution = found_solution.clone();
        let total_hashes = total_hashes.clone();
        let threshold = threshold.clone();
        let thread_app_state = app_state.clone();

        // 优化4: 启发式搜索策略 - 优先分配给上次成功的线程附近区域
        let start_solution = if last_solution_range > 0 && thread_id == last_successful_thread {
            // 成功线程从上次成功位置附近开始
            let base = last_solution_range.saturating_sub(1000);
            U256::from(base + (thread_id as u64))
        } else {
            // 其他线程正常分布
            U256::from(thread_id as u64)
        };

        let handle = tokio::spawn(async move {
            let mut solution = start_solution;
            let step = U256::from(num_threads as u64);

            // 优化5: 批处理计算，每次处理多个哈希
            const BATCH_SIZE: usize = 16;
            let mut encoded_buffers: Vec<Vec<u8>> = Vec::with_capacity(BATCH_SIZE);
            let mut solutions: Vec<U256> = Vec::with_capacity(BATCH_SIZE);

            // 预分配缓冲区
            for _ in 0..BATCH_SIZE {
                encoded_buffers.push(Vec::with_capacity(buffer_size));
                solutions.push(U256::zero());
            }

            // 每10秒显示线程状态
            let thread_start_time = Instant::now();
            let thread_hashes = Arc::new(AtomicU64::new(0));

            tokio::spawn({
                let thread_hashes = thread_hashes.clone();
                let solution_found = solution_found.clone();
                let thread_app_state = thread_app_state.clone();
                async move {
                    while !solution_found.load(Ordering::Relaxed) {
                        sleep(Duration::from_secs(10)).await;
                        if solution_found.load(Ordering::Relaxed) {
                            break;
                        }

                        let elapsed = thread_start_time.elapsed().as_secs_f64();
                        let hashes = thread_hashes.load(Ordering::Relaxed);
                        let rate = hashes as f64 / elapsed;

                        let thread_status = format!(
                            "任务 #{}: [线程 {}] 哈希速度: {:.2} H/s, 当前位置: {}",
                            task_id, thread_id, rate, solution
                        );
                        thread_app_state
                            .lock()
                            .unwrap()
                            .add_log(thread_status, LogLevel::Info);

                        // 更新任务的哈希率
                        thread_app_state
                            .lock()
                            .unwrap()
                            .update_task_hash_rate(task_id, rate);
                    }
                }
            });

            // 计算哈希直到找到解决方案
            while !solution_found.load(Ordering::Relaxed) {
                for i in 0..BATCH_SIZE {
                    // 保存当前尝试的解
                    solutions[i] = solution;

                    // 清空缓冲区
                    encoded_buffers[i].clear();

                    // 编码当前解
                    solidity_pack_bytes_uint_into(&prefix, solution, &mut encoded_buffers[i])?;

                    // 增加计数并更新解
                    solution = solution.saturating_add(step);
                }

                // 批量处理哈希值
                for i in 0..BATCH_SIZE {
                    let hash = keccak256(&encoded_buffers[i]);
                    let _hash_uint = U256::from_big_endian(&hash);

                    // 添加到总哈希计数
                    thread_hashes.fetch_add(1, Ordering::Relaxed);
                    total_hashes.fetch_add(1, Ordering::Relaxed);

                    // 检查是否找到解决方案
                    let hash_biguint = BigUint::from_bytes_be(&hash);
                    if hash_biguint <= threshold {
                        // 找到了有效解
                        found_solution.store(solutions[i].as_u64(), Ordering::Relaxed);
                        solution_found.store(true, Ordering::Relaxed);

                        // 记录成功的线程和解决方案范围，用于下次优化
                        LAST_SUCCESSFUL_THREAD.store(thread_id, Ordering::Relaxed);
                        LAST_SOLUTION_RANGE.store(solutions[i].as_u64(), Ordering::Relaxed);

                        // 更新应用状态
                        let success_msg = format!(
                            "任务 #{}: [线程 {}] 找到了有效解: {}",
                            task_id, thread_id, solutions[i]
                        );
                        thread_app_state
                            .lock()
                            .unwrap()
                            .add_log(success_msg, LogLevel::Success);

                        return Ok::<_, anyhow::Error>(solutions[i]);
                    }

                    // 如果有其他线程找到了解，退出
                    if solution_found.load(Ordering::Relaxed) {
                        break;
                    }
                }

                // 避免CPU过度使用，偶尔让出时间片
                if total_hashes.load(Ordering::Relaxed) % 10000 == 0 {
                    tokio::task::yield_now().await;
                }
            }

            Ok::<_, anyhow::Error>(U256::zero())
        });

        handles.push(handle);
    }

    // 显示整体挖矿进度的任务
    let progress_app_state = app_state.clone();
    let progress_total_hashes = total_hashes.clone();
    let progress_solution_found = solution_found.clone();

    let progress_handle = tokio::spawn(async move {
        let update_interval = Duration::from_secs(5);
        let progress_start_time = Instant::now();

        while !progress_solution_found.load(Ordering::Relaxed) {
            sleep(update_interval).await;

            if progress_solution_found.load(Ordering::Relaxed) {
                break;
            }

            let elapsed = progress_start_time.elapsed().as_secs_f64();
            let hashes = progress_total_hashes.load(Ordering::Relaxed);
            let rate = hashes as f64 / elapsed;

            let progress_msg = format!(
                "任务 #{}: 总计算: {} 哈希, 速度: {:.2} H/s, 运行时间: {:.1}s",
                task_id, hashes, rate, elapsed
            );

            // 获取当前活跃任务数
            let active_tasks = {
                let app = progress_app_state.lock().unwrap();
                app.mining_status.active_tasks
            };

            // 更新日志和挖矿状态
            {
                let mut app = progress_app_state.lock().unwrap();
                app.add_log(progress_msg, LogLevel::Info);
                app.update_mining_status(active_tasks, rate);
            }
        }
    });

    // 等待任何一个线程找到解决方案
    let mut result = U256::zero();
    let mut result_set = false;

    // 收集结果，任何一个正确的结果都可以
    for handle in handles {
        match handle.await {
            Ok(Ok(sol)) if !sol.is_zero() => {
                result = sol;
                result_set = true;
            }
            Ok(Err(e)) => {
                app_state.lock().unwrap().add_log(
                    format!("任务 #{}: 线程出错: {}", task_id, e),
                    LogLevel::Error,
                );
            }
            Err(e) => {
                app_state.lock().unwrap().add_log(
                    format!("任务 #{}: 线程失败: {}", task_id, e),
                    LogLevel::Error,
                );
            }
            _ => {}
        }
    }

    // 确保进度显示线程结束
    let _ = progress_handle.await;

    // 计算和显示总结信息
    let elapsed = start_time.elapsed();
    let total_hash_count = total_hashes.load(Ordering::Relaxed);
    let rate = total_hash_count as f64 / elapsed.as_secs_f64();

    let summary_msg = format!(
        "任务 #{}: 挖矿完成 - 总计算: {} 哈希, 平均速度: {:.2} H/s, 总时间: {:.1}s",
        task_id,
        total_hash_count,
        rate,
        elapsed.as_secs_f64()
    );
    app_state
        .lock()
        .unwrap()
        .add_log(summary_msg, LogLevel::Success);

    if result_set {
        // 找到了有效解
        return Ok(result);
    } else if let Some(sol) =
        U256::from_dec_str(&found_solution.load(Ordering::Relaxed).to_string()).ok()
    {
        // 使用原子变量中存储的解
        return Ok(sol);
    }

    // 不应该到达这里，因为至少有一个线程应该找到解
    Err(anyhow!("任务 #{}: 无法找到有效的挖矿解决方案", task_id))
}

// 优化的内存复用版本solidity_pack_bytes_uint
fn solidity_pack_bytes_uint_into(bytes: &[u8], num: U256, output: &mut Vec<u8>) -> Result<()> {
    // 确保有足够的容量
    let required_capacity = bytes.len() + 32;
    if output.capacity() < required_capacity {
        output.reserve(required_capacity - output.capacity());
    }

    // 添加bytes，保持原始长度
    output.extend_from_slice(bytes);

    // 添加uint256，固定32字节长度
    let mut buffer = [0u8; 32];
    num.to_big_endian(&mut buffer);
    output.extend_from_slice(&buffer);

    Ok(())
}

async fn handle_mining_error(error: anyhow::Error, retry_count: &mut usize) -> Result<()> {
    eprintln!("{}", format!("挖矿错误 / Mining error: {}", error).red());

    *retry_count += 1;
    if *retry_count >= MAX_RETRIES {
        return Err(anyhow!(
            "达到最大重试次数，程序退出 / Max retries reached, exiting."
        ));
    }

    println!(
        "{}",
        format!(
            "5秒后重试（第 {}/{} 次） / Retrying in 5 seconds (Attempt {}/{})",
            retry_count, MAX_RETRIES, retry_count, MAX_RETRIES
        )
        .yellow()
    );

    sleep(Duration::from_secs(5)).await;
    Ok(())
}

// 替换旧的encode_packed函数，添加与JavaScript一致的实现
// 特定的solidityPack实现，对应JS版本中的ethers.utils.solidityPack(['uint256', 'address'], [nonce, address])
fn solidity_pack_uint_address(num: U256, addr: Address) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(32 + 20);

    // 添加uint256，固定32字节长度
    let mut buffer = [0u8; 32];
    num.to_big_endian(&mut buffer);
    result.extend_from_slice(&buffer);

    // 添加address，20字节
    result.extend_from_slice(addr.as_bytes());

    Ok(result)
}

// 特定的solidityPack实现，对应JS版本中的ethers.utils.solidityPack(['bytes', 'uint256'], [prefix, solution])
fn solidity_pack_bytes_uint(bytes: Vec<u8>, num: U256) -> Result<Vec<u8>> {
    let mut result = Vec::with_capacity(bytes.len() + 32);

    // 添加bytes，保持原始长度
    result.extend_from_slice(&bytes);

    // 添加uint256，固定32字节长度
    let mut buffer = [0u8; 32];
    num.to_big_endian(&mut buffer);
    result.extend_from_slice(&buffer);

    Ok(result)
}

// 保留原函数，但只用于其他场景
fn encode_packed(tokens: &[Token]) -> Result<Vec<u8>> {
    let mut result = Vec::new();

    for token in tokens {
        match token {
            Token::Address(addr) => {
                result.extend_from_slice(addr.as_bytes());
            }
            Token::Uint(value) => {
                let mut buffer = [0u8; 32];
                value.to_big_endian(&mut buffer);

                // 跳过前面的零
                let mut start = 0;
                while start < 32 && buffer[start] == 0 {
                    start += 1;
                }

                if start == 32 {
                    // 如果值为0，则添加单个0字节
                    result.push(0);
                } else {
                    // 否则添加非零部分
                    result.extend_from_slice(&buffer[start..]);
                }
            }
            Token::Bytes(bytes) => {
                result.extend_from_slice(bytes);
            }
            _ => {
                return Err(anyhow!("不支持的类型 / Unsupported type"));
            }
        }
    }

    Ok(result)
}
