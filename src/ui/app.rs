use chrono::{DateTime, Local};
use colored::Colorize;
use ethers::types::{Address, U256};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

// 日志消息结构体
#[derive(Clone, Debug)]
pub struct LogMessage {
    pub timestamp: DateTime<Local>,
    pub message: String,
    pub level: LogLevel,
}

// 日志级别
#[derive(Clone, Debug, PartialEq)]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

// 任务信息结构体
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: usize,
    pub nonce: Option<U256>,
    pub difficulty: Option<U256>,
    pub status: String,
    pub hash_rate: Option<f64>,
    pub solution: Option<U256>,
}

// 挖矿状态
#[derive(Clone, Debug)]
pub struct MiningStatus {
    pub active_tasks: usize,
    pub total_tasks: usize,
    pub total_solutions_found: usize,
    pub total_hash_rate: f64,
    pub uptime: u64,      // 以秒为单位
    pub total_mined: f64, // 总计挖矿获得的MAG
}

// 任务处理耗时统计
#[derive(Clone, Debug)]
pub struct TaskTimingStats {
    pub avg_task_request_time_ms: f64,    // 平均任务获取耗时(毫秒)
    pub avg_calculation_time_sec: f64,    // 平均计算耗时(秒)
    pub avg_submission_time_sec: f64,     // 平均提交耗时(秒)
    pub total_tasks_requested: usize,     // 总请求任务数
    pub total_tasks_calculated: usize,    // 总计算完成的任务数
    pub total_tasks_submitted: usize,     // 总提交的任务数
    pub total_requests_failed: usize,     // 总任务请求失败数
    pub total_calculations_failed: usize, // 总计算失败数
    pub total_submissions_failed: usize,  // 总提交失败数
    pub last_updated: DateTime<Local>,    // 最后更新时间
}

impl Default for TaskTimingStats {
    fn default() -> Self {
        Self {
            avg_task_request_time_ms: 0.0,
            avg_calculation_time_sec: 0.0,
            avg_submission_time_sec: 0.0,
            total_tasks_requested: 0,
            total_tasks_calculated: 0,
            total_tasks_submitted: 0,
            total_requests_failed: 0,
            total_calculations_failed: 0,
            total_submissions_failed: 0,
            last_updated: Local::now(),
        }
    }
}

// 应用状态
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum AppState {
    Running,
    Exiting,
}

// 配置选项结构体
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub task_count: usize,      // 并行任务数量
    pub thread_count: usize,    // 每个任务使用的线程数
    pub auto_select_rpc: bool,  // 是否自动选择RPC节点
    pub auto_scroll_logs: bool, // 是否自动滚动日志
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            task_count: 3,                     // 默认3个并行任务
            thread_count: num_cpus::get() / 2, // 默认使用一半的CPU核心数
            auto_select_rpc: true,             // 默认自动选择RPC
            auto_scroll_logs: true,            // 默认自动滚动日志
        }
    }
}

// 主应用结构体
pub struct App {
    // 应用状态
    pub state: AppState,
    // 应用配置
    pub config: AppConfig,
    // 钱包信息
    pub wallet_address: Option<Address>,
    pub wallet_balance: Option<f64>,
    // 初始钱包余额，用于计算挖矿收益
    pub initial_wallet_balance: Option<f64>,
    // 合约信息
    pub contract_address: Option<Address>,
    pub contract_balance: Option<f64>,
    // 挖矿任务
    pub tasks: Vec<TaskInfo>,
    // 挖矿状态
    pub mining_status: MiningStatus,
    // 任务耗时统计
    pub timing_stats: TaskTimingStats,
    // 日志
    pub logs: VecDeque<LogMessage>,
    // 日志容量
    pub max_logs: usize,
    // 当前选择的RPC节点
    pub current_rpc: Option<String>,
    // RPC节点响应时间（毫秒）
    pub rpc_response_times: HashMap<String, u64>,
    // 日志滚动位置
    pub log_scroll: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::Running,
            config: AppConfig::default(),
            wallet_address: None,
            wallet_balance: None,
            initial_wallet_balance: None,
            contract_address: None,
            contract_balance: None,
            tasks: Vec::new(),
            mining_status: MiningStatus {
                active_tasks: 0,
                total_tasks: 0,
                total_solutions_found: 0,
                total_hash_rate: 0.0,
                uptime: 0,
                total_mined: 0.0,
            },
            timing_stats: TaskTimingStats::default(),
            logs: VecDeque::new(),
            max_logs: 1000,
            current_rpc: None,
            rpc_response_times: HashMap::new(),
            log_scroll: 0,
        }
    }

    // 添加日志
    pub fn add_log(&mut self, message: String, level: LogLevel) {
        let log = LogMessage {
            timestamp: Local::now(),
            message,
            level,
        };
        self.logs.push_back(log);

        // 限制日志数量
        if self.logs.len() > self.max_logs {
            self.logs.pop_front();
        }

        // 如果开启了自动滚动，将滚动位置设置为最新
        if self.config.auto_scroll_logs {
            self.log_scroll = 0;
        }
    }

    // 更新任务状态
    pub fn update_task(&mut self, task_id: usize, status: String) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = status;
        } else {
            // 新建任务
            let task = TaskInfo {
                id: task_id,
                nonce: None,
                difficulty: None,
                status,
                hash_rate: None,
                solution: None,
            };
            self.tasks.push(task);
        }
    }

    // 更新任务数据
    pub fn update_task_data(&mut self, task_id: usize, nonce: U256, difficulty: U256) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.nonce = Some(nonce);
            task.difficulty = Some(difficulty);
        } else {
            // 新建任务
            let task = TaskInfo {
                id: task_id,
                nonce: Some(nonce),
                difficulty: Some(difficulty),
                status: "处理中".to_string(),
                hash_rate: None,
                solution: None,
            };
            self.tasks.push(task);
        }
    }

    // 更新任务哈希率
    pub fn update_task_hash_rate(&mut self, task_id: usize, hash_rate: f64) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.hash_rate = Some(hash_rate);
        }
    }

    // 更新任务解决方案
    pub fn update_task_solution(&mut self, task_id: usize, solution: U256) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.solution = Some(solution);
        }
    }

    // 更新钱包信息
    pub fn update_wallet_info(&mut self, address: Address, balance: f64) {
        // 如果是首次更新余额，则设置初始余额
        if self.wallet_balance.is_none() && self.initial_wallet_balance.is_none() {
            self.initial_wallet_balance = Some(balance);
        }

        // 更新钱包地址和当前余额
        self.wallet_address = Some(address);
        self.wallet_balance = Some(balance);

        // 使用钱包余额变化来计算挖矿总量
        self.update_total_mined_from_balance();
    }

    // 使用钱包余额变化计算挖矿总量
    pub fn update_total_mined_from_balance(&mut self) {
        if let (Some(current_balance), Some(initial_balance)) =
            (self.wallet_balance, self.initial_wallet_balance)
        {
            // 如果当前余额高于初始余额，则差额即为挖矿所得
            if current_balance > initial_balance {
                self.mining_status.total_mined = current_balance - initial_balance;

                // 添加日志记录挖矿收益更新
                let mining_msg = format!(
                    "根据钱包余额变化更新挖矿收益: {:.4} MAG",
                    self.mining_status.total_mined
                );
                self.add_log(mining_msg, LogLevel::Info);
            }
        }
    }

    // 更新合约信息
    pub fn update_contract_info(&mut self, address: Address, balance: f64) {
        self.contract_address = Some(address);
        self.contract_balance = Some(balance);
    }

    // 更新挖矿状态
    pub fn update_mining_status(&mut self, active_tasks: usize, total_hash_rate: f64) {
        self.mining_status.active_tasks = active_tasks;
        self.mining_status.total_hash_rate = total_hash_rate;
        // 不再在这里增加uptime
    }

    // 增加运行时间
    pub fn increment_uptime(&mut self) {
        self.mining_status.uptime += 1; // 增加1秒
    }

    // 增加解决方案计数 - 不再更新挖矿收益，只更新解决方案计数
    pub fn add_solution_found(&mut self) {
        self.mining_status.total_solutions_found += 1;
        // 不再直接增加挖矿收益，而是通过钱包余额对比来计算
    }

    // 更新RPC节点响应时间
    pub fn update_rpc_response_time(&mut self, rpc_url: String, response_time_ms: u64) {
        self.rpc_response_times.insert(rpc_url, response_time_ms);
    }

    // 获取最快的RPC节点
    pub fn get_fastest_rpc<'a>(&self, available_rpcs: &[&'a str]) -> Option<&'a str> {
        if self.rpc_response_times.is_empty() {
            return None;
        }

        available_rpcs
            .iter()
            .filter(|rpc| self.rpc_response_times.contains_key(&rpc.to_string()))
            .min_by_key(|rpc| {
                self.rpc_response_times
                    .get(&rpc.to_string())
                    .unwrap_or(&u64::MAX)
            })
            .copied()
    }

    // 滚动日志
    pub fn scroll_logs(&mut self, delta: isize) {
        if delta < 0 && self.log_scroll > 0 {
            self.log_scroll = self.log_scroll.saturating_sub(delta.unsigned_abs());
        } else if delta > 0 {
            self.log_scroll = self.log_scroll.saturating_add(delta as usize);
        }
    }

    // 重置日志滚动位置
    pub fn reset_log_scroll(&mut self) {
        self.log_scroll = 0;
    }

    // 设置配置
    pub fn set_config(&mut self, config: AppConfig) {
        self.config = config;
    }

    // 添加任务请求耗时记录
    pub fn record_task_request_time(&mut self, time_ms: f64, success: bool) {
        if success {
            // 更新平均请求时间（加权平均）
            let total_requests = self.timing_stats.total_tasks_requested as f64;
            self.timing_stats.avg_task_request_time_ms =
                (self.timing_stats.avg_task_request_time_ms * total_requests + time_ms)
                    / (total_requests + 1.0);
            self.timing_stats.total_tasks_requested += 1;
        } else {
            self.timing_stats.total_requests_failed += 1;
        }
        self.timing_stats.last_updated = Local::now();
    }

    // 添加任务计算耗时记录
    pub fn record_calculation_time(&mut self, time_sec: f64, success: bool) {
        if success {
            // 更新平均计算时间（加权平均）
            let total_calcs = self.timing_stats.total_tasks_calculated as f64;
            self.timing_stats.avg_calculation_time_sec =
                (self.timing_stats.avg_calculation_time_sec * total_calcs + time_sec)
                    / (total_calcs + 1.0);
            self.timing_stats.total_tasks_calculated += 1;
        } else {
            self.timing_stats.total_calculations_failed += 1;
        }
        self.timing_stats.last_updated = Local::now();
    }

    // 添加任务提交耗时记录
    pub fn record_submission_time(&mut self, time_sec: f64, success: bool) {
        if success {
            // 更新平均提交时间（加权平均）
            let total_submissions = self.timing_stats.total_tasks_submitted as f64;
            self.timing_stats.avg_submission_time_sec =
                (self.timing_stats.avg_submission_time_sec * total_submissions + time_sec)
                    / (total_submissions + 1.0);
            self.timing_stats.total_tasks_submitted += 1;
        } else {
            self.timing_stats.total_submissions_failed += 1;
        }
        self.timing_stats.last_updated = Local::now();
    }

    // 清理旧任务
    pub fn clean_old_tasks(&mut self, max_tasks: usize) {
        // 首先保留最新的计算任务和当前正在提交的任务
        let mut active_tasks: Vec<TaskInfo> = self
            .tasks
            .iter()
            .filter(|t| {
                t.status == "计算中"
                    || t.status == "处理中"
                    || t.status == "请求中"
                    || t.status.contains("后台提交")
                    || t.status == "提交中"
            })
            .cloned()
            .collect();

        // 找出最近完成的任务(成功或失败)
        let mut completed_tasks: Vec<TaskInfo> = self
            .tasks
            .iter()
            .filter(|t| t.status == "成功" || t.status == "提交失败" || t.status == "错误")
            .cloned()
            .collect();

        // 按ID排序，保留最新的
        completed_tasks.sort_by(|a, b| b.id.cmp(&a.id));

        // 只保留指定数量的已完成任务
        completed_tasks.truncate(max_tasks);

        // 合并活跃任务和最近完成的任务
        active_tasks.extend(completed_tasks);

        // 更新任务列表
        self.tasks = active_tasks;
    }
}
