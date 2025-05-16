use chrono::{DateTime, Local};
use ethers::types::{Address, U256};
use std::collections::VecDeque;

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
    pub uptime: u64, // 以秒为单位
}

// 应用状态
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum AppState {
    Running,
    Exiting,
}

// 主应用结构体
pub struct App {
    // 应用状态
    pub state: AppState,
    // 钱包信息
    pub wallet_address: Option<Address>,
    pub wallet_balance: Option<f64>,
    // 合约信息
    pub contract_address: Option<Address>,
    pub contract_balance: Option<f64>,
    // 挖矿任务
    pub tasks: Vec<TaskInfo>,
    // 挖矿状态
    pub mining_status: MiningStatus,
    // 日志
    pub logs: VecDeque<LogMessage>,
    // 日志容量
    pub max_logs: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::Running,
            wallet_address: None,
            wallet_balance: None,
            contract_address: None,
            contract_balance: None,
            tasks: Vec::new(),
            mining_status: MiningStatus {
                active_tasks: 0,
                total_tasks: 0,
                total_solutions_found: 0,
                total_hash_rate: 0.0,
                uptime: 0,
            },
            logs: VecDeque::new(),
            max_logs: 1000,
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
        self.wallet_address = Some(address);
        self.wallet_balance = Some(balance);
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
        self.mining_status.uptime += 1; // 每次更新增加1秒
    }

    // 增加解决方案计数
    pub fn add_solution_found(&mut self) {
        self.mining_status.total_solutions_found += 1;
    }
}
