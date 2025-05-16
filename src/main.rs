use anyhow::{anyhow, Result};
use colored::*;
use dialoguer::{Input, Select};
use ethers::{
    abi::Token,
    prelude::*,
    providers::{Http, Provider},
    utils::keccak256,
};
use futures::future::join_all;
use futures_util::future::FutureExt;
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
mod rpc;
mod ui;

use contract::MiningContract;
use rpc::{find_fastest_rpc, select_rpc_node, RPC_OPTIONS};
use ui::{create_app, destroy_tui, init_tui, render, App, AppState, Event, LogLevel};

// 定义常量
const CONTRACT_ADDRESS: &str = "0x51e0ab7f7db4a2bf4500dfa59f7a4957afc8c02e";
const MIN_WALLET_BALANCE: f64 = 0.1;
const MIN_CONTRACT_BALANCE: f64 = 3.0;
const MAX_RETRIES: usize = 5;
const MINING_TIMEOUT_SECS: u64 = 600; // 10分钟
                                      // MagnetChain的chainId
const CHAIN_ID: u64 = 114514; // 修正为正确的链ID

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化应用状态
    let app_state = create_app();

    print_welcome_message();

    // 选择RPC节点
    let rpc_url = select_rpc_node(&app_state).await?;
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

    // 配置挖矿参数
    configure_mining_parameters(&app_state);

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

// 更新钱包余额函数
async fn update_wallet_balance<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    app_state: &Arc<Mutex<App>>,
) -> Result<()> {
    // 获取钱包客户端
    let wallet = contract.client();

    // 获取钱包余额
    let balance = wallet.get_balance(wallet.address(), None).await?;
    let balance_eth = ethers::utils::format_ether(balance);
    let balance_f64 = balance_eth.parse::<f64>().unwrap_or(0.0);

    // 更新应用状态中的钱包信息
    let wallet_address = wallet.address();
    app_state
        .lock()
        .unwrap()
        .update_wallet_info(wallet_address, balance_f64);

    // 添加日志
    let balance_msg = format!("钱包余额已更新: {} MAG", balance_f64);
    app_state
        .lock()
        .unwrap()
        .add_log(balance_msg, LogLevel::Info);

    Ok(())
}

async fn start_mining_loop<M: Middleware + 'static>(
    contract: MiningContract<SignerMiddleware<M, LocalWallet>>,
    app_state: Arc<Mutex<App>>,
) -> Result<()> {
    // 获取可用的CPU核心数作为最大线程数
    let cpu_count = num_cpus::get();
    let max_threads = app_state.lock().unwrap().config.thread_count;

    // 更新应用状态 - 只有一个活跃任务
    app_state.lock().unwrap().mining_status.total_tasks = 1;
    app_state.lock().unwrap().mining_status.active_tasks = 1;

    // 启动钱包余额定期更新任务
    let balance_update_contract = contract.clone();
    let balance_update_app_state = app_state.clone();
    tokio::spawn(async move {
        // 每60秒更新一次钱包余额
        let update_interval = Duration::from_secs(60);
        loop {
            sleep(update_interval).await;

            // 尝试更新钱包余额
            match update_wallet_balance(&balance_update_contract, &balance_update_app_state).await {
                Ok(_) => {
                    let msg = "定期更新钱包余额成功".to_string();
                    balance_update_app_state
                        .lock()
                        .unwrap()
                        .add_log(msg, LogLevel::Info);
                }
                Err(e) => {
                    let error_msg = format!("定期更新钱包余额失败: {}", e);
                    balance_update_app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg, LogLevel::Error);
                }
            }
        }
    });

    // 任务ID计数器
    let mut task_id = 0;

    // 创建提交队列
    let (submit_tx, mut submit_rx) = tokio::sync::mpsc::channel::<(usize, U256)>(10);

    // 启动提交处理任务
    let submit_contract = contract.clone();
    let submit_app_state = app_state.clone();
    tokio::spawn(async move {
        while let Some((submit_task_id, solution)) = submit_rx.recv().await {
            let result_msg = format!("后台处理 - 提交任务 {} 的解决方案", submit_task_id);
            submit_app_state
                .lock()
                .unwrap()
                .add_log(result_msg, LogLevel::Info);

            // 更新任务状态为提交中
            submit_app_state
                .lock()
                .unwrap()
                .update_task(submit_task_id, "后台提交中".to_string());

            // 尝试提交解决方案
            match submit_solution(
                &submit_contract,
                submit_task_id,
                solution,
                &submit_app_state,
            )
            .await
            {
                Ok(_) => {
                    // 提交成功
                    let success_msg = format!("任务 {} 后台提交成功", submit_task_id);
                    submit_app_state
                        .lock()
                        .unwrap()
                        .add_log(success_msg, LogLevel::Success);

                    // 更新钱包余额
                    if let Err(e) = update_wallet_balance(&submit_contract, &submit_app_state).await
                    {
                        let error_msg = format!("提交后更新钱包余额失败: {}", e);
                        submit_app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);
                    }
                }
                Err(e) => {
                    // 提交失败
                    let error_msg = format!("任务 {} 后台提交失败: {}", submit_task_id, e);
                    submit_app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg, LogLevel::Error);
                }
            }
        }
    });

    // 单任务串行处理循环 - 不等待提交结果
    loop {
        // 创建新的挖矿任务 - 不等待提交完成
        let submit_tx_clone = submit_tx.clone();
        let mine_result =
            mine_calculate_only(&contract, task_id, &app_state, submit_tx_clone).await;

        // 处理挖矿结果
        match mine_result {
            Ok(()) => {
                let success_msg = format!("任务 {} 计算完成，后台提交中", task_id);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(success_msg, LogLevel::Success);
            }
            Err(e) => {
                let error_msg = format!("任务 {} 计算失败: {}", task_id, e);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(error_msg, LogLevel::Error);

                // 短暂延迟后再尝试新任务（只在失败时延迟）
                sleep(Duration::from_millis(200)).await;
            }
        }

        // 增加任务ID以准备下一个任务
        task_id += 1;

        // 不添加延迟，立即开始下一个任务
    }
}

// 专注于挖矿计算的函数，计算完成后将结果发送到提交队列
async fn mine_calculate_only<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    task_id: usize,
    app_state: &Arc<Mutex<App>>,
    submit_tx: tokio::sync::mpsc::Sender<(usize, U256)>,
) -> Result<()> {
    let mut retry_count = 0;

    loop {
        let task_start_msg = format!("任务 {} 开始请求挖矿任务", task_id);
        app_state
            .lock()
            .unwrap()
            .add_log(task_start_msg, LogLevel::Info);

        match request_and_calculate(contract, task_id, app_state).await {
            Ok(solution) => {
                let task_complete_msg =
                    format!("任务 {} 计算完成，解决方案已发送到提交队列", task_id);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(task_complete_msg, LogLevel::Success);

                // 更新任务解决方案
                app_state
                    .lock()
                    .unwrap()
                    .update_task_solution(task_id, solution);

                // 增加解决方案计数
                app_state.lock().unwrap().add_solution_found();

                // 发送到提交队列 - 不等待结果
                let _ = submit_tx.send((task_id, solution)).await;

                return Ok(());
            }
            Err(e) => {
                if retry_count >= MAX_RETRIES {
                    let max_retry_msg = format!("任务 {} 达到最大重试次数，放弃", task_id);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(max_retry_msg, LogLevel::Warning);

                    return Err(e);
                }

                retry_count += 1;
                let retry_msg = format!(
                    "任务 {} 失败 (重试 {}/{}): {}",
                    task_id, retry_count, MAX_RETRIES, e
                );
                app_state
                    .lock()
                    .unwrap()
                    .add_log(retry_msg, LogLevel::Warning);

                // 等待一段时间后重试
                sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// 请求任务并计算解决方案
async fn request_and_calculate<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    task_id: usize,
    app_state: &Arc<Mutex<App>>,
) -> Result<U256> {
    // 更新任务状态
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "请求中".to_string());

    // 请求挖矿任务
    let tx_request = contract.request_mining_task();
    let pending_tx = tx_request.send().await?;
    let receipt = pending_tx.await?;

    if receipt.is_none() {
        return Err(anyhow!("交易确认失败"));
    }

    // 获取挖矿任务
    let (nonce, difficulty, active) = contract.get_my_task().call().await?;

    if !active {
        return Err(anyhow!("挖矿任务未激活"));
    }

    // 更新任务数据
    app_state
        .lock()
        .unwrap()
        .update_task_data(task_id, nonce, difficulty);

    let task_info_msg = format!(
        "任务 {} - 获取到挖矿任务: nonce={:?}, difficulty={:?}",
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

    // 计算解决方案
    let address = contract.client().address();
    let solution = await_solution(nonce, address, difficulty, task_id, app_state.clone()).await?;

    let solution_msg = format!("任务 {} - 找到解决方案: {:?}", task_id, solution);
    app_state
        .lock()
        .unwrap()
        .add_log(solution_msg, LogLevel::Success);

    // 返回解决方案供后续提交
    Ok(solution)
}

// 提交解决方案函数
async fn submit_solution<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    task_id: usize,
    solution: U256,
    app_state: &Arc<Mutex<App>>,
) -> Result<()> {
    // 提交解决方案
    let submit_request = contract.submit_mining_result(solution);
    let pending_tx = submit_request.send().await?;
    let receipt = pending_tx.await?;

    if receipt.is_none() {
        return Err(anyhow!("提交解决方案的交易确认失败"));
    }

    // 更新任务状态
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "成功".to_string());

    Ok(())
}

async fn await_solution(
    nonce: U256,
    address: Address,
    difficulty: U256,
    task_id: usize,
    app_state: Arc<Mutex<App>>,
) -> Result<U256> {
    let start_time = Instant::now();
    let _timeout = Duration::from_secs(MINING_TIMEOUT_SECS);

    let solution = mine_solution(nonce, address, difficulty, task_id, app_state.clone()).await?;

    let elapsed = start_time.elapsed();
    let mining_time_msg = format!(
        "任务 {} - 挖矿耗时: {:.2}秒",
        task_id,
        elapsed.as_secs_f64()
    );
    app_state
        .lock()
        .unwrap()
        .add_log(mining_time_msg.clone(), LogLevel::Info);

    Ok(solution)
}

async fn mine_solution(
    nonce: U256,
    address: Address,
    difficulty: U256,
    task_id: usize,
    app_state: Arc<Mutex<App>>,
) -> Result<U256> {
    // 使用所有配置的线程
    let thread_count = app_state.lock().unwrap().config.thread_count;

    // 记录所有可用线程数
    let all_threads_info = format!("任务 {} 使用 {} 个线程同时挖矿", task_id, thread_count);
    app_state
        .lock()
        .unwrap()
        .add_log(all_threads_info, LogLevel::Info);

    // 创建停止标志
    let stop_flag = Arc::new(AtomicBool::new(false));
    let solution_found = Arc::new(AtomicBool::new(false));
    let solution_value = Arc::new(AtomicU64::new(0));
    let hashes_checked = Arc::new(AtomicUsize::new(0));

    // 启动哈希率更新任务
    let update_task = {
        let hashes_checked = hashes_checked.clone();
        let app_state = app_state.clone();
        let solution_found = solution_found.clone();
        let stop_flag = stop_flag.clone();

        tokio::spawn(async move {
            let mut last_check = Instant::now();
            let mut last_hashes = 0;

            while !solution_found.load(Ordering::Relaxed) && !stop_flag.load(Ordering::Relaxed) {
                sleep(Duration::from_secs(1)).await;

                let current_hashes = hashes_checked.load(Ordering::Relaxed);
                let elapsed = last_check.elapsed();
                let hash_rate = (current_hashes - last_hashes) as f64 / elapsed.as_secs_f64();

                // 更新哈希率
                app_state
                    .lock()
                    .unwrap()
                    .update_task_hash_rate(task_id, hash_rate);

                // 更新总哈希率
                app_state.lock().unwrap().update_mining_status(1, hash_rate);

                last_check = Instant::now();
                last_hashes = current_hashes;
            }
        })
    };

    // 准备工作区间 - 将整个空间均匀分配给所有线程
    let batch_size = u64::MAX / thread_count as u64;
    let mut handles = Vec::with_capacity(thread_count);

    // 将地址和nonce打包
    let packed_data = solidity_pack_uint_address(nonce, address)?;

    // 启动多个计算线程
    for i in 0..thread_count {
        let start = U256::from(i as u64 * batch_size);
        let end = if i == thread_count - 1 {
            U256::from(u64::MAX)
        } else {
            U256::from((i + 1) as u64 * batch_size - 1)
        };

        let packed_data = packed_data.clone();
        let stop_flag = stop_flag.clone();
        let solution_found = solution_found.clone();
        let solution_value = solution_value.clone();
        let hashes_checked = hashes_checked.clone();
        let difficulty = difficulty;

        // 启动计算线程
        let handle = std::thread::spawn(move || {
            // 使用BigUint进行范围计算 - 修复U256没有to_be_bytes方法的问题
            let mut current = BigUint::from_bytes_be(&to_be_bytes_32(&start));
            let end_big = BigUint::from_bytes_be(&to_be_bytes_32(&end));
            let difficulty_big = BigUint::from_bytes_be(&to_be_bytes_32(&difficulty));
            // 创建一个最大值的BigUint，避免在循环中移动
            let max_big = BigUint::from_bytes_be(&[0xFF; 32]);

            let mut counter = 0;
            const REPORT_INTERVAL: usize = 1000;

            while current <= end_big && !stop_flag.load(Ordering::Relaxed) {
                // 将当前值转换为bytes
                let current_bytes = current.to_bytes_be();
                let current_u256 = U256::from_big_endian(&pad_to_32(&current_bytes));

                // 打包数据
                let mut input_data = packed_data.clone();
                solidity_pack_bytes_uint_into(&[], current_u256, &mut input_data)?;

                // 计算哈希
                let hash = keccak256(input_data);
                let _hash_u256 = U256::from_big_endian(&hash);
                let hash_big = BigUint::from_bytes_be(&hash);

                // 检查是否满足难度要求 - 使用克隆避免移动问题
                if hash_big <= max_big.clone() / difficulty_big.clone() {
                    // 找到解决方案
                    solution_value.store(current_u256.as_u64(), Ordering::Relaxed);
                    solution_found.store(true, Ordering::Relaxed);
                    stop_flag.store(true, Ordering::Relaxed);
                    break;
                }

                // 更新计数器和检查的哈希数
                counter += 1;
                if counter % REPORT_INTERVAL == 0 {
                    hashes_checked.fetch_add(REPORT_INTERVAL, Ordering::Relaxed);
                }

                // 递增当前值
                current += 1u32;
            }

            // 添加剩余的计数
            hashes_checked.fetch_add(counter % REPORT_INTERVAL, Ordering::Relaxed);

            Ok::<(), anyhow::Error>(())
        });

        handles.push(handle);
    }

    // 等待任何一个线程找到解决方案或所有线程完成
    loop {
        // 检查是否找到解决方案
        if solution_found.load(Ordering::Relaxed) {
            // 停止所有线程
            stop_flag.store(true, Ordering::Relaxed);
            break;
        }

        // 检查是否所有线程都已完成
        let all_finished = handles.iter().all(|h| h.is_finished());
        if all_finished {
            break;
        }

        // 等待一会儿再检查
        sleep(Duration::from_millis(100)).await;
    }

    // 取消哈希率更新任务
    update_task.abort();

    // 等待所有线程完成
    for handle in handles {
        if let Err(e) = handle.join() {
            eprintln!("计算线程异常退出: {:?}", e);
        }
    }

    // 检查是否找到了解决方案
    if solution_found.load(Ordering::Relaxed) {
        let solution = U256::from(solution_value.load(Ordering::Relaxed));
        return Ok(solution);
    }

    Err(anyhow!("未能找到解决方案"))
}

// 将字节数组填充到32字节
fn pad_to_32(bytes: &[u8]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let start = 32 - bytes.len();
    result[start..].copy_from_slice(bytes);
    result
}

// 将U256转换为固定长度的字节数组
fn to_be_bytes_32(value: &U256) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    value.to_big_endian(&mut bytes);
    bytes
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

// 配置挖矿参数
fn configure_mining_parameters(app_state: &Arc<Mutex<App>>) {
    // 默认值
    let default_thread_count = app_state.lock().unwrap().config.thread_count;
    let cpu_count = num_cpus::get();

    // 说明挖矿模式
    println!(
        "{}",
        "配置挖矿参数 / Configure Mining Parameters:".bold().cyan()
    );

    // 说明新的挖矿模式
    println!(
        "{}",
        "使用单任务多线程模式 - 所有线程集中处理一个任务以提高效率"
            .bold()
            .green()
    );
    println!("系统检测到 {} 个CPU核心", cpu_count);
    println!(
        "当前线程数: {}，建议值：{}，线程数越多挖矿速度越快",
        default_thread_count, cpu_count
    );

    // 线程数量
    let thread_count: usize = Input::new()
        .with_prompt("输入挖矿使用的线程数 / Enter number of mining threads")
        .default(default_thread_count)
        .validate_with(|input: &usize| -> Result<(), &str> {
            if *input > 0 && *input <= cpu_count * 2 {
                Ok(())
            } else {
                Err(&"线程数必须大于0且不超过CPU核心数的两倍 / Thread count must be > 0 and <= 2x CPU cores")
            }
        })
        .interact()
        .unwrap_or(default_thread_count);

    // 更新配置 - 保持任务数为1
    let mut app = app_state.lock().unwrap();
    app.config.task_count = 1; // 固定为1个任务
    app.config.thread_count = thread_count;

    let config_msg = format!(
        "已设置挖矿配置 - 使用 {} 个线程全力处理单个任务",
        thread_count
    );
    println!("{}", config_msg.green());
    app.add_log(config_msg, LogLevel::Info);
}
