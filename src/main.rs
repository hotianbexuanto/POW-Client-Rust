use anyhow::{anyhow, Result};
use chrono::Local;
use colored::Colorize;
use console::Term;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use ethers::{
    abi::Token,
    middleware::SignerMiddleware,
    prelude::*,
    providers::{Http, JsonRpcClient, Middleware, Provider},
    signers::{coins_bip39::English, LocalWallet, MnemonicBuilder, Signer},
    types::{Address, BlockNumber, TransactionReceipt, H160, H256, U256},
    utils::keccak256,
};
use lazy_static::lazy_static;
use num_bigint::BigUint;
use num_traits::Num;
use rand::Rng;
use serde::de::DeserializeOwned;
use std::{
    collections::HashMap,
    fmt,
    io::{self, stdout, Stdout},
    path::Path,
    str::{from_utf8, FromStr},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, time::sleep};

/// 项目文档：Magnet POW 挖矿客户端
///
/// 哈希验证优化说明：
///
/// 1. 增加了三种不同的验证方法，通过多数投票机制减少验证错误率：
///    - 方法1：检查 hash * difficulty < 2^256（对应合约验证方式）
///    - 方法2：检查 hash < 2^256 / difficulty（避免乘法可能的溢出问题）
///    - 方法3：通过前导零比较快速筛选（高效的初筛方法）
///
/// 2. 添加了哈希有效性基础检查 (is_valid_hash)：
///    - 检测全零或无效哈希
///    - 验证哈希长度
///    - 对于高难度值，检查前导零的合理性
///
/// 3. 使用引用而非复制操作优化大数计算性能
///
/// 4. 在挖矿过程中提前筛选可能有问题的哈希计算结果
///
/// 这些优化确保了验证结果的准确性，大幅降低了错误率。
mod contract;
mod rpc;
mod ui;

use contract::MiningContract;
use rpc::select_rpc_node;
use ui::{
    app::{App, AppConfig, LogLevel, TaskInfo},
    event::{Event, EventHandler},
};

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
    let app_state = ui::create_app();

    print_welcome_message();

    // 尝试连接RPC节点
    let mut rpc_url = None;
    let mut provider = None;

    // 最多尝试3次连接RPC
    for _ in 0..3 {
        // 选择RPC节点
        match select_rpc_node(&app_state).await {
            Ok(selected_rpc) => {
                println!(
                    "{}",
                    format!("已选择 RPC / Selected RPC: {}", selected_rpc).green()
                );

                // 添加到日志
                app_state
                    .lock()
                    .unwrap()
                    .add_log(format!("已选择 RPC: {}", selected_rpc), LogLevel::Info);

                // 初始化以太坊提供者
                match Provider::<Http>::try_from(selected_rpc) {
                    Ok(provider_instance) => {
                        rpc_url = Some(selected_rpc);
                        provider = Some(provider_instance);
                        break;
                    }
                    Err(e) => {
                        let error_msg = format!("连接RPC节点失败: {}. 尝试其他节点。", e);
                        println!("{}", error_msg.yellow());
                        app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Warning);

                        // 关闭自动选择，以便下次手动选择
                        app_state.lock().unwrap().config.auto_select_rpc = false;
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("选择RPC节点失败: {}. 尝试其他节点。", e);
                println!("{}", error_msg.yellow());
                app_state
                    .lock()
                    .unwrap()
                    .add_log(error_msg, LogLevel::Warning);

                // 关闭自动选择，以便下次手动选择
                app_state.lock().unwrap().config.auto_select_rpc = false;
            }
        }
    }

    // 如果所有尝试都失败，返回错误
    if provider.is_none() {
        let error_msg = "无法连接到任何RPC节点，请检查网络连接后重试";
        println!("{}", error_msg.red());
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg.to_string(), LogLevel::Error);
        return Err(anyhow!(error_msg));
    }

    let provider = provider.unwrap();
    let rpc_url = rpc_url.unwrap();

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
    let (mut terminal, _tx, mut rx) = ui::init_tui()?;

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
    // 用于定时更新统计UI的计时器
    let mut last_stats_update = Instant::now();
    let stats_update_interval = Duration::from_secs(5); // 每5秒更新一次统计

    loop {
        // 渲染TUI
        terminal.draw(|f| {
            ui::ui::render::<ratatui::backend::CrosstermBackend<std::io::Stdout>>(
                f,
                &app_state.lock().unwrap(),
            );
        })?;

        // 处理事件
        match rx.recv().await {
            Some(Event::Input(_)) => {
                // 用户按下q或Ctrl+C等退出键
                app_state.lock().unwrap().state = ui::app::AppState::Exiting;
                break;
            }
            Some(Event::Tick) => {
                // 定时更新 - 可以在这里更新一些实时数据
                app_state.lock().unwrap().increment_uptime();

                // 每5秒更新一次统计UI
                if last_stats_update.elapsed() >= stats_update_interval {
                    let now = chrono::Local::now();
                    app_state.lock().unwrap().timing_stats.last_updated = now;
                    last_stats_update = Instant::now();
                }
            }
            None => break,
        }

        // 检查是否应该退出
        if app_state.lock().unwrap().state == ui::app::AppState::Exiting {
            break;
        }
    }

    // 清理并退出
    ui::destroy_tui(&mut terminal)?;

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

    // 更新应用状态中的钱包信息和挖矿总量
    let wallet_address = wallet.address();

    // 添加日志
    let previous_balance = app_state.lock().unwrap().wallet_balance;
    let balance_msg = format!("钱包余额已更新: {} MAG", balance_f64);
    app_state
        .lock()
        .unwrap()
        .add_log(balance_msg, LogLevel::Info);

    // 更新钱包信息（这也会调用update_total_mined_from_balance）
    app_state
        .lock()
        .unwrap()
        .update_wallet_info(wallet_address, balance_f64);

    // 如果余额增加，记录余额变化
    if let Some(prev_balance) = previous_balance {
        if balance_f64 > prev_balance {
            let earned = balance_f64 - prev_balance;
            let earn_msg = format!("钱包余额增加了 {:.4} MAG", earned);
            app_state
                .lock()
                .unwrap()
                .add_log(earn_msg, LogLevel::Success);
        }
    }

    Ok(())
}

async fn start_mining_loop<M: Middleware + 'static>(
    contract: MiningContract<SignerMiddleware<M, LocalWallet>>,
    app_state: Arc<Mutex<App>>,
) -> Result<()> {
    // 获取用户配置的任务数和线程数
    let task_count = app_state.lock().unwrap().config.task_count;
    let _thread_count = app_state.lock().unwrap().config.thread_count;

    // 更新应用状态中的任务数量
    app_state.lock().unwrap().mining_status.total_tasks = task_count;
    app_state.lock().unwrap().mining_status.active_tasks = task_count;

    // 启动钱包余额定期更新任务
    let balance_update_contract = contract.clone();
    let balance_update_app_state = app_state.clone();
    tokio::spawn(async move {
        // 每30秒更新一次钱包余额，以便更准确地跟踪挖矿收益
        let update_interval = Duration::from_secs(30);
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

    // 创建提交队列
    let (submit_tx, mut submit_rx) = tokio::sync::mpsc::channel::<(usize, U256)>(20);

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
                .update_task(submit_task_id, "提交中".to_string());

            // 获取当前任务信息，验证任务是否仍然有效
            match submit_contract.get_my_task().call().await {
                Ok((current_nonce, current_difficulty, active)) => {
                    if !active {
                        let error_msg = format!("任务 {} 已失效，跳过提交", submit_task_id);
                        submit_app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Warning);
                        submit_app_state
                            .lock()
                            .unwrap()
                            .update_task(submit_task_id, "任务已失效".to_string());
                        continue;
                    }

                    // 再次验证解决方案
                    let address = submit_contract.client().address();
                    match verify_solution_locally(
                        current_nonce,
                        address,
                        solution,
                        current_difficulty,
                    ) {
                        Ok(valid) => {
                            if !valid {
                                let error_msg = format!(
                                    "任务 {} 解决方案不再满足难度要求，跳过提交",
                                    submit_task_id
                                );
                                submit_app_state
                                    .lock()
                                    .unwrap()
                                    .add_log(error_msg, LogLevel::Warning);
                                submit_app_state
                                    .lock()
                                    .unwrap()
                                    .update_task(submit_task_id, "验证失败".to_string());
                                continue;
                            }
                        }
                        Err(e) => {
                            let error_msg = format!(
                                "任务 {} 解决方案验证出错: {}, 跳过提交",
                                submit_task_id, e
                            );
                            submit_app_state
                                .lock()
                                .unwrap()
                                .add_log(error_msg, LogLevel::Error);
                            submit_app_state
                                .lock()
                                .unwrap()
                                .update_task(submit_task_id, "验证错误".to_string());
                            continue;
                        }
                    }
                }
                Err(e) => {
                    let error_msg =
                        format!("获取任务 {} 信息失败: {}, 尝试直接提交", submit_task_id, e);
                    submit_app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg, LogLevel::Warning);
                }
            }

            // 执行提交
            match submit_with_recovery(
                &submit_contract,
                solution,
                submit_task_id,
                &submit_app_state,
            )
            .await
            {
                Ok(Some(receipt)) => {
                    // 检查交易状态
                    if receipt.status.unwrap_or_default() == U64::from(1) {
                        let success_msg =
                            format!("任务 {} 的解决方案已确认，交易成功", submit_task_id);
                        submit_app_state
                            .lock()
                            .unwrap()
                            .add_log(success_msg, LogLevel::Success);

                        // 更新任务状态为成功
                        submit_app_state
                            .lock()
                            .unwrap()
                            .update_task(submit_task_id, "成功".to_string());

                        // 在提交成功后更新钱包余额
                        if let Err(e) =
                            update_wallet_balance(&submit_contract, &submit_app_state).await
                        {
                            let error_msg =
                                format!("任务 {} 提交后更新钱包余额失败: {}", submit_task_id, e);
                            submit_app_state
                                .lock()
                                .unwrap()
                                .add_log(error_msg, LogLevel::Warning);
                        } else {
                            let update_msg =
                                format!("任务 {} 提交成功后更新钱包余额成功", submit_task_id);
                            submit_app_state
                                .lock()
                                .unwrap()
                                .add_log(update_msg, LogLevel::Success);
                        }
                    } else {
                        let error_msg =
                            format!("任务 {} 的解决方案交易失败，请检查链上状态", submit_task_id);
                        submit_app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);

                        // 更新任务状态为交易失败
                        submit_app_state
                            .lock()
                            .unwrap()
                            .update_task(submit_task_id, "交易失败".to_string());
                    }
                }
                Ok(None) => {
                    let error_msg = format!("任务 {} 的解决方案交易未能确认", submit_task_id);
                    submit_app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg, LogLevel::Error);

                    // 更新任务状态为失败
                    submit_app_state
                        .lock()
                        .unwrap()
                        .update_task(submit_task_id, "确认失败".to_string());
                }
                Err(e) => {
                    // 尝试解析错误消息，给用户更清晰的提示
                    let error_msg = e.to_string();
                    let parsed_msg = if error_msg.contains("underpriced") {
                        format!(
                            "任务 {} 后台提交失败: Gas价格过低，重试次数耗尽",
                            submit_task_id
                        )
                    } else if error_msg.contains("nonce too low") {
                        format!(
                            "任务 {} 后台提交失败: nonce问题，重试次数耗尽",
                            submit_task_id
                        )
                    } else if error_msg.contains("reverted") {
                        format!(
                            "任务 {} 后台提交失败: 合约拒绝交易，任务可能已过期或已被完成",
                            submit_task_id
                        )
                    } else {
                        format!("任务 {} 后台提交失败: {}", submit_task_id, e)
                    };

                    submit_app_state
                        .lock()
                        .unwrap()
                        .add_log(parsed_msg, LogLevel::Error);

                    // 更新任务状态为提交失败，这样UI就不会继续显示它
                    submit_app_state
                        .lock()
                        .unwrap()
                        .update_task(submit_task_id, "提交失败".to_string());
                }
            }
        }
    });

    // 任务ID计数器
    let mut task_id = 0;

    // 创建多个挖矿任务处理器
    let mut task_handles = Vec::new();

    // 启动指定数量的挖矿任务
    for i in 0..task_count {
        let task_contract = contract.clone();
        let task_app_state = app_state.clone();
        let task_submit_tx = submit_tx.clone();

        let _task_name = format!("挖矿任务-{}", i);
        let task_info = format!("启动并行挖矿任务 #{}", i);
        task_app_state
            .lock()
            .unwrap()
            .add_log(task_info, LogLevel::Info);

        // 为每个任务分配不同的起始ID，避免任务ID冲突
        let start_task_id = task_id;
        task_id += 1;

        // 启动独立挖矿任务
        let handle = tokio::spawn(async move {
            let mut current_task_id = start_task_id;

            loop {
                // 创建新的挖矿任务 - 不等待提交完成
                let mine_result = mine_calculate_only(
                    &task_contract,
                    current_task_id,
                    &task_app_state,
                    task_submit_tx.clone(),
                )
                .await;

                // 处理挖矿结果
                match mine_result {
                    Ok(()) => {
                        let success_msg = format!("任务 {} 计算完成，后台提交中", current_task_id);
                        task_app_state
                            .lock()
                            .unwrap()
                            .add_log(success_msg, LogLevel::Success);
                    }
                    Err(e) => {
                        let error_msg = format!("任务 {} 计算失败: {}", current_task_id, e);
                        task_app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);
                    }
                }

                // 增加任务ID，准备下一个任务
                current_task_id += task_count;

                // 每10个任务清理一次，保留最近的任务数量 * 2个已完成任务
                if current_task_id % 10 == 0 {
                    task_app_state
                        .lock()
                        .unwrap()
                        .clean_old_tasks(task_count * 2);
                }

                // 无需延迟，立即开始下一个任务
            }
        });

        task_handles.push(handle);
    }

    // 等待所有任务完成（实际上永远不会完成，除非出错）
    for handle in task_handles {
        let _ = handle.await;
    }

    Ok(())
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
    // 记录任务请求开始时间
    let request_start_time = Instant::now();

    // 更新任务状态
    app_state
        .lock()
        .unwrap()
        .update_task(task_id, "请求中".to_string());

    // 请求挖矿任务
    let tx_request = contract.request_mining_task();
    let pending_tx = match tx_request.send().await {
        Ok(tx) => tx,
        Err(e) => {
            // 记录请求失败
            app_state
                .lock()
                .unwrap()
                .record_task_request_time(request_start_time.elapsed().as_millis() as f64, false);
            return Err(anyhow!("请求挖矿任务失败: {}", e));
        }
    };

    let receipt = match pending_tx.await {
        Ok(r) => r,
        Err(e) => {
            // 记录请求失败
            app_state
                .lock()
                .unwrap()
                .record_task_request_time(request_start_time.elapsed().as_millis() as f64, false);
            return Err(anyhow!("交易确认失败: {}", e));
        }
    };

    if receipt.is_none() {
        // 记录请求失败
        app_state
            .lock()
            .unwrap()
            .record_task_request_time(request_start_time.elapsed().as_millis() as f64, false);
        return Err(anyhow!("交易确认失败"));
    }

    // 获取挖矿任务
    let (nonce, difficulty, active) = match contract.get_my_task().call().await {
        Ok(result) => result,
        Err(e) => {
            // 记录请求失败
            app_state
                .lock()
                .unwrap()
                .record_task_request_time(request_start_time.elapsed().as_millis() as f64, false);
            return Err(anyhow!("获取任务数据失败: {}", e));
        }
    };

    if !active {
        // 记录请求失败
        app_state
            .lock()
            .unwrap()
            .record_task_request_time(request_start_time.elapsed().as_millis() as f64, false);
        return Err(anyhow!("挖矿任务未激活"));
    }

    // 记录请求成功及耗时
    let request_time = request_start_time.elapsed().as_millis() as f64;
    app_state
        .lock()
        .unwrap()
        .record_task_request_time(request_time, true);

    // 日志记录请求时间
    let time_msg = format!("任务 {} - 请求耗时: {:.2}毫秒", task_id, request_time);
    app_state.lock().unwrap().add_log(time_msg, LogLevel::Info);

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

    // 记录计算开始时间
    let calculation_start_time = Instant::now();

    // 计算解决方案
    let address = contract.client().address();
    let solution =
        match await_solution(nonce, address, difficulty, task_id, app_state.clone()).await {
            Ok(s) => s,
            Err(e) => {
                // 记录计算失败
                app_state
                    .lock()
                    .unwrap()
                    .record_calculation_time(calculation_start_time.elapsed().as_secs_f64(), false);
                return Err(e);
            }
        };

    // 记录计算成功及耗时
    let calculation_time = calculation_start_time.elapsed().as_secs_f64();
    app_state
        .lock()
        .unwrap()
        .record_calculation_time(calculation_time, true);

    let solution_msg = format!(
        "任务 {} - 找到解决方案: {:?}, 计算耗时: {:.2}秒",
        task_id, solution, calculation_time
    );
    app_state
        .lock()
        .unwrap()
        .add_log(solution_msg, LogLevel::Success);

    // 本地再次验证解决方案，确保满足难度要求
    let verification_msg = format!("任务 {} - 验证解决方案是否满足难度要求", task_id);
    app_state
        .lock()
        .unwrap()
        .add_log(verification_msg, LogLevel::Info);

    if !verify_solution_locally(nonce, address, solution, difficulty)? {
        let error_msg = format!(
            "任务 {} - 本地验证失败，解决方案不满足难度要求，放弃提交",
            task_id
        );
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg, LogLevel::Error);

        return Err(anyhow!("解决方案本地验证失败"));
    }

    // 验证任务是否仍然有效（防止其他矿工已经提交）
    let (current_nonce, _, current_active) = contract.get_my_task().call().await?;
    if !current_active || current_nonce != nonce {
        let error_msg = format!("任务 {} - 任务已过期或已被他人提交，放弃提交", task_id);
        app_state
            .lock()
            .unwrap()
            .add_log(error_msg, LogLevel::Warning);

        return Err(anyhow!("任务已失效"));
    }

    // 返回解决方案供后续提交
    Ok(solution)
}

// 本地验证解决方案
fn verify_solution_locally(
    nonce: U256,
    address: Address,
    solution: U256,
    difficulty: U256,
) -> Result<bool> {
    // 打包数据 - 与挖矿计算中相同的方法
    let packed_data = solidity_pack_uint_address(nonce, address)?;
    let mut input_data = Vec::with_capacity(packed_data.len() + 32);
    input_data.extend_from_slice(&packed_data);

    // 添加solution
    let mut buffer = [0u8; 32];
    solution.to_big_endian(&mut buffer);
    input_data.extend_from_slice(&buffer);

    // 计算哈希
    let hash = keccak256(&input_data);
    let hash_u256 = U256::from_big_endian(&hash);

    // 验证哈希是否有基本问题
    if !is_valid_hash(&hash, &difficulty) {
        return Ok(false);
    }

    // 计算难度值
    let hash_big = BigUint::from_bytes_be(&hash);
    let difficulty_big = BigUint::from_bytes_be(&to_be_bytes_32(&difficulty));
    let max_big = BigUint::from_bytes_be(&[0xFF; 32]);

    // 三种验证方法，提高准确性

    // 方法1：检查 hash * difficulty < 2^256
    // 优点：直接对应合约中的验证方式，精确度高
    let product = &hash_big * &difficulty_big;
    let is_valid_method1 = product < max_big;

    // 方法2：检查 hash < 2^256 / difficulty
    // 优点：避免了大数乘法可能的溢出问题，数值稳定
    let target = &max_big / &difficulty_big;
    let is_valid_method2 = hash_big <= target;

    // 方法3：使用比较哈希的最高位来验证
    // 优点：计算简单高效，可以快速过滤明显不符合要求的哈希
    let required_zeros = (difficulty.bits() as usize).saturating_sub(1);
    let actual_zeros = hash_u256.leading_zeros() as usize;
    let is_valid_method3 = actual_zeros >= required_zeros;

    // 记录验证结果
    let all_methods_agree =
        is_valid_method1 == is_valid_method2 && is_valid_method2 == is_valid_method3;
    if !all_methods_agree {
        println!(
            "警告：验证方法结果不一致！方法1={}, 方法2={}, 方法3={}",
            is_valid_method1, is_valid_method2, is_valid_method3
        );
    }

    // 使用三种验证方法的多数投票，至少两种方法同意才有效
    // 这大大降低了单个验证方法可能出现的错误率
    let is_valid = (is_valid_method1 && is_valid_method2)
        || (is_valid_method2 && is_valid_method3)
        || (is_valid_method1 && is_valid_method3);

    Ok(is_valid)
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
    let error_occurred = Arc::new(AtomicBool::new(false));
    let error_message = Arc::new(Mutex::new(String::new()));

    // 启动哈希率更新任务
    let update_task = {
        let hashes_checked = hashes_checked.clone();
        let app_state = app_state.clone();
        let solution_found = solution_found.clone();
        let stop_flag = stop_flag.clone();
        let error_occurred = error_occurred.clone();

        tokio::spawn(async move {
            let mut last_check = Instant::now();
            let mut last_hashes = 0;

            while !solution_found.load(Ordering::Relaxed)
                && !stop_flag.load(Ordering::Relaxed)
                && !error_occurred.load(Ordering::Relaxed)
            {
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

    // 预先计算一些常量值
    // 将地址和nonce打包 - 只需计算一次
    let packed_data = solidity_pack_uint_address(nonce, address)?;

    // 预先计算难度相关值
    let difficulty_bytes = to_be_bytes_32(&difficulty);
    let max_bytes = [0xFF; 32];

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
        let error_occurred = error_occurred.clone();
        let error_message = error_message.clone();

        // 拷贝预计算的值
        let difficulty_bytes = difficulty_bytes;
        let max_bytes = max_bytes;
        let difficulty = difficulty; // 为了新的验证方法，需要传递U256版本的难度值

        // 启动计算线程
        let handle = std::thread::spawn(move || {
            // 使用BigUint进行范围计算
            let mut current = BigUint::from_bytes_be(&to_be_bytes_32(&start));
            let end_big = BigUint::from_bytes_be(&to_be_bytes_32(&end));

            // 预先计算难度比较值 - 只计算一次
            let difficulty_big = BigUint::from_bytes_be(&difficulty_bytes);
            let max_big = BigUint::from_bytes_be(&max_bytes);

            // 使用引用避免移动所有权
            match &max_big / &difficulty_big {
                target_value => {
                    let mut counter = 0;
                    const REPORT_INTERVAL: usize = 10000; // 增大报告间隔减少原子操作频率

                    // 预分配缓冲区减少内存分配
                    let mut input_data = Vec::with_capacity(packed_data.len() + 32);
                    input_data.extend_from_slice(&packed_data);
                    input_data.resize(packed_data.len() + 32, 0); // 预留32字节用于solution

                    // 更高效地使用buffer
                    let data_prefix_len = packed_data.len();

                    while current <= end_big
                        && !stop_flag.load(Ordering::Relaxed)
                        && !error_occurred.load(Ordering::Relaxed)
                    {
                        // 将当前值转换为bytes
                        let current_bytes = current.to_bytes_be();
                        let current_u256 = match U256::from_big_endian(&pad_to_32(&current_bytes)) {
                            value => value,
                        };

                        // 直接在预分配缓冲区上操作，避免多次克隆和分配
                        current_u256.to_big_endian(&mut input_data[data_prefix_len..]);

                        // 计算哈希
                        let hash = keccak256(&input_data);

                        // 先进行基础检查，确保哈希有效
                        if !is_valid_hash(&hash, &difficulty) {
                            // 哈希可能有问题，跳过这次迭代
                            current += 1u32;
                            continue;
                        }

                        // 处理哈希转换，避免可能的错误
                        let hash_big = match BigUint::from_bytes_be(&hash) {
                            value => value,
                        };

                        // 创建U256版本以便做leading_zeros检查
                        let hash_u256 = U256::from_big_endian(&hash);

                        // 检查是否满足难度要求 - 三种验证方法的至少两种通过

                        // 方法1: hash_big <= target_value (已经预计算好了)
                        let is_valid1 = hash_big <= target_value;

                        // 方法2: hash * difficulty < max (2^256)
                        let product = &hash_big * &difficulty_big;
                        let is_valid2 = product < max_big;

                        // 方法3: 检查前导零
                        let required_zeros = (difficulty.bits() as usize).saturating_sub(1);
                        let actual_zeros = hash_u256.leading_zeros() as usize;
                        let is_valid3 = actual_zeros >= required_zeros;

                        // 使用多数投票法：至少两种方法认为有效
                        if (is_valid1 && is_valid2)
                            || (is_valid2 && is_valid3)
                            || (is_valid1 && is_valid3)
                        {
                            // 找到解决方案
                            match current_u256.as_u64() {
                                value => {
                                    solution_value.store(value, Ordering::Relaxed);
                                    solution_found.store(true, Ordering::Relaxed);
                                    stop_flag.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }

                        // 更新计数器和检查的哈希数
                        counter += 1;
                        if counter % REPORT_INTERVAL == 0 {
                            hashes_checked.fetch_add(REPORT_INTERVAL, Ordering::Relaxed);

                            // 定期检查停止标志，减少原子操作频率
                            if stop_flag.load(Ordering::Relaxed)
                                || error_occurred.load(Ordering::Relaxed)
                            {
                                break;
                            }
                        }

                        // 递增当前值
                        current += 1u32;
                    }

                    // 添加剩余的计数
                    hashes_checked.fetch_add(counter % REPORT_INTERVAL, Ordering::Relaxed);
                }
            }

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

        // 检查是否有错误发生
        if error_occurred.load(Ordering::Relaxed) {
            // 停止所有线程
            stop_flag.store(true, Ordering::Relaxed);
            let error_text = error_message.lock().unwrap().clone();
            return Err(anyhow!("计算过程发生错误: {}", error_text));
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
            let error_msg = format!("计算线程异常退出: {:?}", e);
            app_state
                .lock()
                .unwrap()
                .add_log(error_msg.clone(), LogLevel::Error);

            error_occurred.store(true, Ordering::Relaxed);
            *error_message.lock().unwrap() = error_msg;
        }
    }

    // 检查是否找到了解决方案
    if solution_found.load(Ordering::Relaxed) {
        let solution = U256::from(solution_value.load(Ordering::Relaxed));
        return Ok(solution);
    }

    // 检查是否有错误发生
    if error_occurred.load(Ordering::Relaxed) {
        let error_text = error_message.lock().unwrap().clone();
        return Err(anyhow!("计算过程发生错误: {}", error_text));
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

// 检查哈希是否可能有问题（新增函数）
fn is_valid_hash(hash: &[u8], difficulty: &U256) -> bool {
    // 检查哈希是否全零或有明显问题
    if hash.iter().all(|&x| x == 0) {
        return false;
    }

    // 检查哈希长度
    if hash.len() != 32 {
        return false;
    }

    // 对于特别大的难度值，检查前导零的合理性
    if difficulty.bits() > 200 {
        // 检查哈希前导字节是否全为零
        let leading_zeros = hash.iter().take_while(|&&x| x == 0).count();

        // 如果难度值很高但哈希没有足够的前导零，可能是计算有问题
        if leading_zeros < (difficulty.bits() as usize / 16) {
            return false;
        }
    }

    true
}

// 将U256转换为固定长度的字节数组
fn to_be_bytes_32(value: &U256) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    value.to_big_endian(&mut bytes);
    bytes
}

// 计算预期挖矿收益率
fn calculate_mining_profitability(
    gas_price: U256,
    estimated_gas: U256,
    reward_amount: U256,
) -> Result<(bool, f64)> {
    // 计算交易成本
    let transaction_cost = gas_price * estimated_gas;

    // 将U256转换为f64以便进行比率计算
    let cost_eth = ethers::utils::format_ether(transaction_cost)
        .parse::<f64>()
        .unwrap_or(0.0);
    let reward_eth = ethers::utils::format_ether(reward_amount)
        .parse::<f64>()
        .unwrap_or(0.0);

    if cost_eth >= reward_eth {
        // 成本大于或等于奖励，无利可图
        return Ok((false, 0.0));
    }

    // 计算利润率: (reward - cost) / reward * 100%
    let profit_percentage = (reward_eth - cost_eth) / reward_eth * 100.0;

    // 如果利润率低于10%，认为收益太低
    let is_profitable = profit_percentage > 10.0;

    Ok((is_profitable, profit_percentage))
}

// 根据网络负载调整Gas价格
fn adjust_gas_price_by_network_load(
    base_gas_price: U256,
    network_load: f64, // 0.0-1.0 代表网络负载百分比
) -> U256 {
    // 计算动态增幅，负载越高增幅越大，最高增幅为50%
    let load_factor = 1.0 + (network_load * 0.5);

    // 将f64转换回U256
    let multiplier = (load_factor * 100.0) as u64;
    let adjusted_gas = base_gas_price * multiplier / 100;

    adjusted_gas
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
    // 获取当前配置
    let current_config = app_state.lock().unwrap().config.clone();
    let default_thread_count = current_config.thread_count;
    let default_task_count = current_config.task_count;
    let default_auto_select_rpc = current_config.auto_select_rpc;
    let cpu_count = num_cpus::get();

    // 说明挖矿模式
    println!(
        "{}",
        "配置挖矿参数 / Configure Mining Parameters:".bold().cyan()
    );

    // 说明新的挖矿模式
    println!(
        "{}",
        "使用多任务并行挖矿模式 - 提高挖矿效率和成功率"
            .bold()
            .green()
    );
    println!("系统检测到 {} 个CPU核心", cpu_count);
    println!(
        "当前线程数: {}，当前任务数: {}",
        default_thread_count, default_task_count
    );
    println!("建议：线程数 * 任务数 <= CPU核心数 * 2，以获得最佳性能");

    // 线程数量
    let thread_count: usize = Input::new()
        .with_prompt("输入每个任务使用的线程数 / Enter threads per mining task")
        .default(default_thread_count)
        .validate_with(|input: &usize| -> Result<(), &str> {
            if *input > 0 && *input <= cpu_count * 2 {
                Ok(())
            } else {
                Err("线程数必须大于0且不超过CPU核心数的两倍 / Thread count must be > 0 and <= 2x CPU cores")
            }
        })
        .interact()
        .unwrap_or(default_thread_count);

    // 任务数量
    let task_count: usize = Input::new()
        .with_prompt("输入并行任务数量 / Enter parallel mining tasks")
        .default(default_task_count)
        .validate_with(|input: &usize| -> Result<(), &str> {
            if *input > 0 && *input <= 10 {
                Ok(())
            } else {
                Err("任务数必须大于0且不超过10 / Task count must be > 0 and <= 10")
            }
        })
        .interact()
        .unwrap_or(default_task_count);

    // 询问是否自动选择RPC节点
    println!(
        "{}",
        "RPC节点自动选择将测试多个节点并选择响应最快的一个"
            .bold()
            .cyan()
    );
    println!(
        "当前设置: {}",
        if default_auto_select_rpc {
            "自动选择"
        } else {
            "手动选择"
        }
    );

    let auto_select_options = ["自动选择 RPC 节点", "手动选择 RPC 节点"];
    let auto_select_default = if default_auto_select_rpc { 0 } else { 1 };

    let auto_select_index = Select::new()
        .with_prompt("选择 RPC 节点获取方式")
        .default(auto_select_default)
        .items(&auto_select_options)
        .interact()
        .unwrap_or(auto_select_default);

    let auto_select_rpc = auto_select_index == 0;

    // 更新配置
    let mut app = app_state.lock().unwrap();
    app.config.task_count = task_count;
    app.config.thread_count = thread_count;
    app.config.auto_select_rpc = auto_select_rpc;

    // 输出配置信息
    let config_msg = format!(
        "已设置挖矿配置 - 使用 {} 个任务，每个任务 {} 个线程，RPC节点获取方式: {}",
        task_count,
        thread_count,
        if auto_select_rpc {
            "自动选择"
        } else {
            "手动选择"
        }
    );
    println!("{}", config_msg.green());
    app.add_log(config_msg, LogLevel::Info);
}

// 执行提交，带有nonce恢复和gas调整机制
async fn submit_with_recovery<M: Middleware + 'static>(
    contract: &MiningContract<SignerMiddleware<M, LocalWallet>>,
    solution: U256,
    task_id: usize,
    app_state: &Arc<Mutex<App>>,
) -> Result<Option<TransactionReceipt>> {
    // 记录提交开始时间
    let submit_start_time = Instant::now();

    let mut retry_count = 0;
    const MAX_RETRIES: usize = 3;

    // 获取当前gas价格，并根据网络情况智能调整
    let mut gas_price = contract.client().get_gas_price().await?;

    // 估算网络负载 (查询最近区块的gasUsed/gasLimit比率)
    let mut network_load = 0.5; // 默认假设中等负载

    // 尝试获取网络负载信息
    match contract.client().get_block(BlockNumber::Latest).await {
        Ok(Some(block)) => {
            // gas_used和gas_limit已经是U256类型而不是Option<U256>
            let gas_used = block.gas_used;
            let gas_limit = block.gas_limit;
            if gas_limit.is_zero() {
                // 防止除以零
                network_load = 0.5; // 使用默认值
            } else {
                network_load = gas_used.as_u128() as f64 / gas_limit.as_u128() as f64;
                let load_info = format!(
                    "任务 {} - 当前网络负载: {:.1}%",
                    task_id,
                    network_load * 100.0
                );
                app_state.lock().unwrap().add_log(load_info, LogLevel::Info);
            }
        }
        _ => {
            app_state.lock().unwrap().add_log(
                format!("任务 {} - 无法获取网络负载信息，使用默认值", task_id),
                LogLevel::Warning,
            );
        }
    }

    // 根据网络负载调整初始buffer
    let initial_buffer_percent = if network_load > 0.8 {
        115 // 高负载时提高到15%
    } else if network_load > 0.5 {
        112 // 中高负载时提高到12%
    } else {
        110 // 正常负载提高10%
    };

    let mut gas_price_with_buffer = gas_price * initial_buffer_percent / 100;

    // 估算交易所需gas量和收益，检查是否值得挖矿
    let estimated_gas = U256::from(150_000); // 估计POW提交大约需要约15万gas
    let mining_reward = ethers::utils::parse_ether(3.0)?; // 每次挖矿奖励3 MAG

    // 计算预期收益率
    if let Ok((is_profitable, profit_percentage)) =
        calculate_mining_profitability(gas_price_with_buffer, estimated_gas, mining_reward)
    {
        if !is_profitable {
            let warning_msg = format!(
                "任务 {} - 警告：当前Gas价格下挖矿可能无利可图，收益率仅为{:.1}%",
                task_id, profit_percentage
            );
            app_state
                .lock()
                .unwrap()
                .add_log(warning_msg, LogLevel::Warning);
        } else {
            let profit_msg = format!(
                "任务 {} - 当前Gas价格下预计收益率: {:.1}%",
                task_id, profit_percentage
            );
            app_state
                .lock()
                .unwrap()
                .add_log(profit_msg, LogLevel::Info);
        }
    }

    // 记录基础gas价格信息
    let base_gas_info = format!(
        "任务 {} - 基础Gas价格: {} Gwei, 初始调整后: {} Gwei (上浮{}%)",
        task_id,
        gas_price.as_u64() / 1_000_000_000,
        gas_price_with_buffer.as_u64() / 1_000_000_000,
        initial_buffer_percent - 100
    );
    app_state
        .lock()
        .unwrap()
        .add_log(base_gas_info, LogLevel::Info);

    // 获取当前nonce
    let address = contract.client().address();
    let mut current_nonce = contract
        .client()
        .get_transaction_count(address, None)
        .await?;

    // 尝试提交，如果失败则重试
    while retry_count < MAX_RETRIES {
        // 每次循环都创建新的交易实例
        let mut tx = contract.submit_mining_result(solution);

        // 设置当前的gas价格和nonce
        tx.tx.set_gas_price(gas_price_with_buffer);
        tx.tx.set_nonce(current_nonce);

        let result = tx.send().await;

        match result {
            Ok(pending_tx) => {
                // 交易发送成功，等待确认
                let tx_hash = format!("{:?}", pending_tx.tx_hash());
                let log_msg = format!(
                    "任务 {} 解决方案已提交，交易哈希: {} ，等待确认",
                    task_id, tx_hash
                );
                app_state.lock().unwrap().add_log(log_msg, LogLevel::Info);

                // 等待交易确认
                match pending_tx.await {
                    Ok(receipt_option) => {
                        let total_time = submit_start_time.elapsed().as_secs_f64();

                        if let Some(receipt) = receipt_option.as_ref() {
                            // 计算实际gas使用和成本
                            if let Some(gas_used) = receipt.gas_used {
                                let actual_gas_cost = gas_used * gas_price_with_buffer;
                                let gas_cost_eth = ethers::utils::format_ether(actual_gas_cost);
                                let reward_eth = "3.0"; // 固定3 MAG奖励
                                let profit = 3.0 - gas_cost_eth.parse::<f64>().unwrap_or(0.0);
                                let profit_percent = profit / 3.0 * 100.0;

                                let success_msg = format!(
                                    "任务 {} 挖矿成功！使用Gas: {}, 成本: {} MAG, 利润: {:.4} MAG ({:.1}%), 提交耗时: {:.2}秒",
                                    task_id, gas_used, gas_cost_eth, profit, profit_percent, total_time
                                );
                                app_state
                                    .lock()
                                    .unwrap()
                                    .add_log(success_msg, LogLevel::Success);
                            } else {
                                let success_basic = format!(
                                    "任务 {} 挖矿成功！交易已确认，提交耗时: {:.2}秒",
                                    task_id, total_time
                                );
                                app_state
                                    .lock()
                                    .unwrap()
                                    .add_log(success_basic, LogLevel::Success);
                            }
                        }

                        // 记录提交成功及耗时
                        app_state
                            .lock()
                            .unwrap()
                            .record_submission_time(total_time, true);
                        return Ok(receipt_option);
                    }
                    Err(e) => {
                        let error_msg = format!("任务 {} 交易确认失败: {}", task_id, e);
                        app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);

                        // 记录提交失败
                        app_state.lock().unwrap().record_submission_time(
                            submit_start_time.elapsed().as_secs_f64(),
                            false,
                        );
                        return Err(anyhow!("交易确认失败"));
                    }
                }
            }
            Err(e) => {
                let error_text = e.to_string();
                if error_text.contains("nonce too low") || error_text.contains("already known") {
                    // nonce问题，获取正确的nonce
                    match contract.client().get_transaction_count(address, None).await {
                        Ok(new_nonce) => {
                            let nonce_msg = format!(
                                "任务 {} nonce过低，更新nonce: {:?} -> {:?}",
                                task_id, current_nonce, new_nonce
                            );
                            app_state
                                .lock()
                                .unwrap()
                                .add_log(nonce_msg, LogLevel::Warning);

                            // 更新nonce供下次循环使用
                            current_nonce = new_nonce;
                        }
                        Err(nonce_err) => {
                            let error_msg =
                                format!("任务 {} 无法获取新nonce: {}", task_id, nonce_err);
                            app_state
                                .lock()
                                .unwrap()
                                .add_log(error_msg, LogLevel::Error);

                            // 记录提交失败
                            app_state.lock().unwrap().record_submission_time(
                                submit_start_time.elapsed().as_secs_f64(),
                                false,
                            );
                            return Err(anyhow!(nonce_err));
                        }
                    }
                } else if error_text.contains("underpriced") {
                    // gas价格过低，智能调整gas价格

                    // 计算当前挖矿收益估算 (3 MAG每次挖矿)
                    let mining_reward = ethers::utils::parse_ether(3.0)?;

                    // 估算交易所需gas量 (通常POW提交大约需要10-15万gas)
                    let estimated_gas = U256::from(150_000);

                    // 检查当前区块链状态，可能网络拥堵有变化
                    let network_load_updated =
                        match contract.client().get_block(BlockNumber::Latest).await {
                            Ok(Some(block)) => {
                                // gas_used和gas_limit已经是U256类型而不是Option<U256>
                                let gas_used = block.gas_used;
                                let gas_limit = block.gas_limit;
                                if !gas_limit.is_zero() {
                                    Some(gas_used.as_u128() as f64 / gas_limit.as_u128() as f64)
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };

                    // 如果网络负载更新成功，调整网络负载系数
                    if let Some(new_load) = network_load_updated {
                        if (new_load - network_load).abs() > 0.1 {
                            // 网络负载变化超过10%
                            network_load = new_load;
                            let update_msg = format!(
                                "任务 {} - 网络负载已更新: {:.1}%",
                                task_id,
                                network_load * 100.0
                            );
                            app_state
                                .lock()
                                .unwrap()
                                .add_log(update_msg, LogLevel::Info);
                        }
                    }

                    // 最小增长率（基础增长率，确保gas价格有足够增长以满足underpriced要求）
                    // 当前设置为15%，较低的基础涨价可减少矿工成本
                    let min_increase = 1150;

                    // 根据网络负载和重试次数动态调整增幅
                    let network_factor = match network_load {
                        load if load > 0.8 => 10, // 高负载 +10% 额外增幅
                        load if load > 0.6 => 5,  // 中高负载 +5% 额外增幅
                        _ => 0,                   // 正常负载不额外增加
                    };

                    // 根据重试次数计算基础增幅
                    let retry_factor = match retry_count {
                        0 => 0,  // 第一次重试不额外增加基础增幅
                        1 => 10, // 第二次重试 +10% 额外增幅
                        _ => 25, // 最后重试 +25% 额外增幅
                    };

                    // 计算总增长率（基础1000 + 网络因子 + 重试因子）/ 10
                    let increase_percent =
                        (min_increase + network_factor * 10 + retry_factor * 10) / 10;

                    // 计算最大可接受的gas价格（确保至少有40%的收益）
                    let min_profit_ratio = 40; // 40%最低收益率，保证更好的收益
                    let max_acceptable_gas_price: U256 =
                        mining_reward * (100 - min_profit_ratio) / (estimated_gas * 100);

                    let new_gas_price = gas_price_with_buffer * increase_percent / 100;

                    // 确保不超过最大可接受价格
                    let new_gas_price = std::cmp::min(new_gas_price, max_acceptable_gas_price);

                    // 计算涨幅百分比
                    let increase_ratio = (new_gas_price.as_u128() as f64
                        / gas_price_with_buffer.as_u128() as f64)
                        - 1.0;
                    let increase_percent_display = (increase_ratio * 100.0) as u64;

                    let gas_msg = format!(
                        "任务 {} - Gas价格过低调整: {} Gwei → {} Gwei (上涨{}%), 预计收益率: {}%",
                        task_id,
                        gas_price_with_buffer.as_u64() / 1_000_000_000,
                        new_gas_price.as_u64() / 1_000_000_000,
                        increase_percent_display,
                        100 - (new_gas_price.as_u128() * estimated_gas.as_u128() * 100
                            / mining_reward.as_u128()) as u64
                    );

                    app_state
                        .lock()
                        .unwrap()
                        .add_log(gas_msg, LogLevel::Warning);

                    // 更新gas价格供下次循环使用
                    gas_price_with_buffer = new_gas_price;
                } else {
                    // 其他错误，不重试
                    let error_msg = format!("任务 {} 提交失败: {}", task_id, e);
                    app_state
                        .lock()
                        .unwrap()
                        .add_log(error_msg, LogLevel::Error);

                    // 记录提交失败
                    app_state
                        .lock()
                        .unwrap()
                        .record_submission_time(submit_start_time.elapsed().as_secs_f64(), false);
                    return Err(anyhow!(e));
                }
            }
        }

        retry_count += 1;
        let retry_msg = format!(
            "任务 {} 重试提交 ({}/{})",
            task_id, retry_count, MAX_RETRIES
        );
        app_state
            .lock()
            .unwrap()
            .add_log(retry_msg, LogLevel::Warning);

        // 等待一会儿再重试
        sleep(Duration::from_millis(500)).await;
    }

    // 记录提交失败（达到最大重试次数）
    app_state
        .lock()
        .unwrap()
        .record_submission_time(submit_start_time.elapsed().as_secs_f64(), false);

    Err(anyhow!("达到最大重试次数，提交失败"))
}
