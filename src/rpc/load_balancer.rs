use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};
use ethers::providers::{Http, Provider};
use ethers::prelude::Middleware;
use rand::seq::SliceRandom;
use rand::Rng;
use crate::ui::app::{App, LogLevel};

// 节点状态跟踪
#[derive(Debug, Clone)]
pub struct NodeStatus {
    pub url: String,
    pub last_response_time: Option<u64>,  // 毫秒
    pub avg_response_time: u64,          // 毫秒
    pub success_count: usize,
    pub fail_count: usize,
    pub last_used: Instant,
    pub health_score: f64,               // 0-100分，100为最佳
    pub consecutive_failures: usize,
    pub is_active: bool,
}

impl NodeStatus {
    pub fn new(url: String) -> Self {
        Self {
            url,
            last_response_time: None,
            avg_response_time: 1000, // 默认假设1000ms
            success_count: 0,
            fail_count: 0,
            last_used: Instant::now(),
            health_score: 50.0,      // 默认中等分数
            consecutive_failures: 0,
            is_active: true,
        }
    }
    
    // 更新节点健康分数
    fn update_health_score(&mut self) {
        // 失败率因子 (0-1)，0表示没有失败
        let failure_factor = if self.success_count + self.fail_count > 0 {
            self.fail_count as f64 / (self.success_count + self.fail_count) as f64
        } else {
            0.0
        };
        
        // 响应时间因子 (0-1)，值越低越好
        let response_time_factor = if self.avg_response_time < 50 {
            0.0  // 非常快的响应
        } else if self.avg_response_time > 2000 {
            1.0  // 非常慢的响应
        } else {
            (self.avg_response_time as f64 - 50.0) / 1950.0
        };
        
        // 连续失败惩罚因子
        let consecutive_failure_penalty = (self.consecutive_failures as f64 * 10.0).min(50.0);
        
        // 计算总健康分数 (0-100)
        self.health_score = 100.0 - 
            (failure_factor * 40.0) -         // 失败率最多扣40分
            (response_time_factor * 30.0) -   // 响应时间最多扣30分
            consecutive_failure_penalty;      // 连续失败最多扣50分
            
        // 确保分数在0-100范围内
        self.health_score = self.health_score.max(0.0).min(100.0);
    }
    
    // 记录成功请求
    pub fn record_success(&mut self, response_time: u64) {
        self.last_response_time = Some(response_time);
        
        // 更新平均响应时间 (指数移动平均)
        if self.success_count == 0 {
            self.avg_response_time = response_time;
        } else {
            // 赋予新数据30%的权重
            self.avg_response_time = (self.avg_response_time * 7 + response_time * 3) / 10;
        }
        
        self.success_count += 1;
        self.consecutive_failures = 0;
        self.last_used = Instant::now();
        self.update_health_score();
    }
    
    // 记录失败请求
    pub fn record_failure(&mut self) {
        self.fail_count += 1;
        self.consecutive_failures += 1;
        self.last_used = Instant::now();
        
        // 如果连续失败超过阈值，则标记为非活跃
        if self.consecutive_failures >= 3 {
            self.is_active = false;
        }
        
        self.update_health_score();
    }
    
    // 重新激活节点
    pub fn reactivate(&mut self) {
        self.is_active = true;
        self.consecutive_failures = 0;
        self.update_health_score();
    }
}

// 负载均衡器
#[derive(Clone)]
pub struct LoadBalancer {
    nodes: HashMap<String, NodeStatus>,
    default_nodes: Vec<String>,
    active_node: Option<String>,
    last_refresh: Instant,
    refresh_interval: Duration,
}

impl LoadBalancer {
    pub fn new(default_nodes: Vec<String>) -> Self {
        let mut nodes = HashMap::new();
        for node in &default_nodes {
            nodes.insert(node.clone(), NodeStatus::new(node.clone()));
        }
        
        Self {
            nodes,
            default_nodes,
            active_node: None,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(60), // 每60秒刷新一次节点状态
        }
    }
    
    // 添加新节点
    pub fn add_node(&mut self, node_url: String) {
        if !self.nodes.contains_key(&node_url) {
            self.nodes.insert(node_url.clone(), NodeStatus::new(node_url));
        }
    }
    
    // 添加多个节点
    pub fn add_nodes(&mut self, node_urls: Vec<String>) {
        for url in node_urls {
            self.add_node(url);
        }
    }
    
    // 获取所有节点状态
    pub fn get_all_nodes(&self) -> Vec<NodeStatus> {
        self.nodes.values().cloned().collect()
    }
    
    // 获取活跃节点
    pub fn get_active_nodes(&self) -> Vec<NodeStatus> {
        self.nodes.values()
            .filter(|node| node.is_active)
            .cloned()
            .collect()
    }
    
    // 记录节点成功
    pub fn record_node_success(&mut self, node_url: &str, response_time: u64) {
        if let Some(node) = self.nodes.get_mut(node_url) {
            node.record_success(response_time);
        }
    }
    
    // 记录节点失败
    pub fn record_node_failure(&mut self, node_url: &str) {
        if let Some(node) = self.nodes.get_mut(node_url) {
            node.record_failure();
        }
    }
    
    // 选择最佳节点 - 加权随机选择
    pub fn select_best_node(&mut self) -> Option<String> {
        // 获取活跃节点
        let active_nodes: Vec<&NodeStatus> = self.nodes.values()
            .filter(|node| node.is_active)
            .collect();
            
        if active_nodes.is_empty() {
            // 如果没有活跃节点，尝试恢复一个默认节点
            self.try_recover_default_node();
            return self.active_node.clone();
        }
        
        // 按健康分数排序，分数高的优先
        let mut weighted_nodes: Vec<(&NodeStatus, f64)> = active_nodes.iter()
            .map(|&node| (node, node.health_score))
            .collect();
        
        if weighted_nodes.is_empty() {
            return None;
        }
        
        // 总权重
        let total_weight: f64 = weighted_nodes.iter().map(|(_, weight)| weight).sum();
        
        if total_weight <= 0.0 {
            // 如果总权重为零，随机选择一个活跃节点
            let mut rng = rand::thread_rng();
            let random_node = active_nodes.choose(&mut rng).unwrap();
            self.active_node = Some(random_node.url.clone());
            return self.active_node.clone();
        }
        
        // 按健康分数进行加权随机选择
        let mut rng = rand::thread_rng();
        let random_value = rng.gen_range(0.0..total_weight);
        
        let mut cumulative_weight = 0.0;
        for (node, weight) in weighted_nodes {
            cumulative_weight += weight;
            if cumulative_weight >= random_value {
                self.active_node = Some(node.url.clone());
                return self.active_node.clone();
            }
        }
        
        // 以防万一，选择第一个节点
        self.active_node = Some(active_nodes[0].url.clone());
        self.active_node.clone()
    }
    
    // 选择最快的节点 - 直接基于响应时间
    pub fn select_fastest_node(&self) -> Option<String> {
        self.nodes.values()
            .filter(|node| node.is_active)
            .min_by_key(|node| node.avg_response_time)
            .map(|node| node.url.clone())
    }
    
    // 尝试恢复一个默认节点
    fn try_recover_default_node(&mut self) {
        for node_url in &self.default_nodes {
            if let Some(node) = self.nodes.get_mut(node_url) {
                node.reactivate();
                self.active_node = Some(node_url.clone());
                return;
            }
        }
    }
    
    // 刷新所有节点状态
    pub async fn refresh_node_status(&mut self, app_state: &Arc<Mutex<App>>) -> Result<()> {
        // 只有在上次刷新超过间隔时间后才刷新
        if self.last_refresh.elapsed() < self.refresh_interval {
            return Ok(());
        }
        
        // 记录刷新时间
        self.last_refresh = Instant::now();
        
        // 尝试重新激活所有不活跃的节点
        for node in self.nodes.values_mut() {
            if !node.is_active {
                node.reactivate();
            }
        }
        
        // 测试所有节点
        let log_msg = "刷新所有RPC节点状态...";
        println!("{}", colored::Colorize::cyan(log_msg));
        app_state
            .lock()
            .unwrap()
            .add_log(log_msg.to_string(), LogLevel::Info);
            
        // 使用单独的函数来测试所有节点
        self.test_all_nodes(app_state).await?;
        
        Ok(())
    }
    
    // 测试所有节点
    async fn test_all_nodes(&mut self, app_state: &Arc<Mutex<App>>) -> Result<()> {
        let mut node_urls: Vec<String> = self.nodes.keys().cloned().collect();
        
        // 并行测试所有节点
        let mut tasks = Vec::new();
        for node_url in node_urls.clone() {
            let task = tokio::spawn(async move {
                let start = Instant::now();
                match Provider::<Http>::try_from(node_url.clone()) {
                    Ok(provider) => {
                        match provider.get_block_number().await {
                            Ok(_) => {
                                let elapsed = start.elapsed();
                                let response_time = elapsed.as_millis() as u64;
                                Some((node_url, response_time))
                            }
                            Err(_) => None,
                        }
                    }
                    Err(_) => None,
                }
            });
            tasks.push(task);
        }
        
        // 等待所有测试完成
        let results = futures::future::join_all(tasks).await;
        
        // 处理结果
        for result in results.into_iter().flatten() {
            match result {
                Some((node_url, response_time)) => {
                    if let Some(node) = self.nodes.get_mut(&node_url) {
                        node.record_success(response_time);
                        
                        // 添加日志
                        let log_msg = format!(
                            "节点 {} 可用，响应时间: {}ms，健康分数: {:.1}", 
                            node_url, response_time, node.health_score
                        );
                        app_state
                            .lock()
                            .unwrap()
                            .add_log(log_msg, LogLevel::Info);
                    }
                }
                None => {
                    // 记录节点失败
                    if let Some(result_index) = node_urls.iter().position(|url| {
                        result.as_ref().map_or(false, |(node_url, _)| node_url == url)
                    }) {
                        let failed_url = &node_urls[result_index];
                        if let Some(node) = self.nodes.get_mut(failed_url) {
                            node.record_failure();
                            
                            // 添加日志
                            let log_msg = format!(
                                "节点 {} 不可用，健康分数: {:.1}", 
                                failed_url, node.health_score
                            );
                            app_state
                                .lock()
                                .unwrap()
                                .add_log(log_msg, LogLevel::Warning);
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    // 智能请求：使用负载均衡器发送请求并自动处理失败
    pub async fn smart_request<F, R>(&mut self, app_state: &Arc<Mutex<App>>, request_func: F) -> Result<R> 
    where
        F: Fn(&str) -> futures::future::BoxFuture<'static, Result<R>>,
        R: Send + 'static,
    {
        // 尝试刷新节点状态
        let _ = self.refresh_node_status(app_state).await;
        
        // 选择最佳节点
        let node_url = self.select_best_node()
            .ok_or_else(|| anyhow!("没有可用的RPC节点"))?;
            
        let start = Instant::now();
        match request_func(&node_url).await {
            Ok(result) => {
                // 记录成功
                let elapsed = start.elapsed().as_millis() as u64;
                self.record_node_success(&node_url, elapsed);
                Ok(result)
            }
            Err(err) => {
                // 记录失败
                self.record_node_failure(&node_url);
                
                // 日志记录失败
                let log_msg = format!("节点 {} 请求失败: {}，尝试备用节点", node_url, err);
                app_state
                    .lock()
                    .unwrap()
                    .add_log(log_msg, LogLevel::Warning);
                
                // 尝试另一个节点
                let backup_node = self.select_best_node()
                    .ok_or_else(|| anyhow!("没有可用的备用RPC节点"))?;
                    
                if backup_node == node_url {
                    return Err(anyhow!("所有RPC节点都不可用"));
                }
                
                // 使用备用节点重试
                let backup_start = Instant::now();
                match request_func(&backup_node).await {
                    Ok(result) => {
                        // 记录备用节点成功
                        let elapsed = backup_start.elapsed().as_millis() as u64;
                        self.record_node_success(&backup_node, elapsed);
                        
                        let success_msg = format!("备用节点 {} 请求成功", backup_node);
                        app_state
                            .lock()
                            .unwrap()
                            .add_log(success_msg, LogLevel::Success);
                            
                        Ok(result)
                    }
                    Err(backup_err) => {
                        // 记录备用节点失败
                        self.record_node_failure(&backup_node);
                        
                        let error_msg = format!("备用节点 {} 也失败: {}", backup_node, backup_err);
                        app_state
                            .lock()
                            .unwrap()
                            .add_log(error_msg, LogLevel::Error);
                            
                        Err(anyhow!("所有尝试的RPC节点都失败"))
                    }
                }
            }
        }
    }
} 