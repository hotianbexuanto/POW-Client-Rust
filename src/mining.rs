use ethers::types::{Address, U256};
use ethers::utils::keccak256;
use num_bigint::BigUint;
use num_traits::Num;
use std::ops::Shl;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

// 挖矿会话结构体
#[derive(Clone)]
pub struct MiningSession {
    pub nonce: U256,
    pub address: Address,
    pub difficulty: U256,
    pub target: BigUint,
    pub start_time: Instant,
    pub hash_counter: Arc<AtomicU64>,
    pub stop_flag: Arc<AtomicBool>,
}

impl MiningSession {
    pub fn new(nonce: U256, address: Address, difficulty: U256) -> Self {
        // 计算目标值: 2^256 / difficulty
        let base: BigUint = BigUint::from(1u32);
        let shifted: BigUint = base.shl(256);
        let diff_bigint = BigUint::from_str_radix(&difficulty.to_string(), 10).unwrap();
        let target = shifted / diff_bigint;

        Self {
            nonce,
            address,
            difficulty,
            target,
            start_time: Instant::now(),
            hash_counter: Arc::new(AtomicU64::new(0)),
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    // 获取当前哈希率
    pub fn get_hashrate(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.hash_counter.load(Ordering::Relaxed) as f64 / elapsed
        } else {
            0.0
        }
    }

    // 获取总哈希计数
    pub fn get_hash_count(&self) -> u64 {
        self.hash_counter.load(Ordering::Relaxed)
    }

    // 停止挖矿
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    // 检查是否应该停止
    pub fn should_stop(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }

    // 计算解决方案
    pub fn find_solution(&self, thread_count: usize) -> Option<U256> {
        // 创建线程池
        let mut handles = Vec::with_capacity(thread_count);
        let chunk_size = u64::MAX / thread_count as u64;

        // 启动线程
        for i in 0..thread_count {
            let start = i as u64 * chunk_size;
            let end = if i == thread_count - 1 {
                u64::MAX
            } else {
                start + chunk_size - 1
            };

            let nonce = self.nonce;
            let address = self.address;
            let target = self.target.clone();
            let hash_counter = Arc::clone(&self.hash_counter);
            let stop_flag = Arc::clone(&self.stop_flag);

            let handle = thread::spawn(move || {
                let mut solution = U256::from(start);
                let end_u256 = U256::from(end);

                while solution <= end_u256 {
                    // 检查是否应该停止
                    if stop_flag.load(Ordering::SeqCst) {
                        return None;
                    }

                    // 增加哈希计数
                    hash_counter.fetch_add(1, Ordering::Relaxed);

                    // 计算哈希
                    let hash = calculate_hash(nonce, address, solution);

                    // 检查哈希是否小于目标值
                    let hash_bigint = BigUint::from_bytes_be(&hash);
                    if hash_bigint <= target {
                        return Some(solution);
                    }

                    // 增加解决方案
                    if solution == U256::MAX {
                        break;
                    }
                    solution += U256::one();

                    // 每1000次哈希检查一次，避免过度阻塞
                    if solution.low_u64() % 1000 == 0 {
                        thread::yield_now();
                    }
                }

                None
            });

            handles.push(handle);
        }

        // 等待任意线程找到解决方案
        let mut solution = None;

        // 简单的轮询检查线程是否完成
        'outer: loop {
            for handle in &handles {
                if handle.is_finished() {
                    // 尝试获取结果
                    match handle.thread().unpark() {
                        // 这只是为了唤醒线程，实际上我们不能直接获取结果
                        // 我们需要等待所有线程完成
                        _ => {}
                    }
                }
            }

            // 检查是否应该停止
            if self.stop_flag.load(Ordering::SeqCst) {
                break;
            }

            // 短暂休眠，避免CPU占用过高
            thread::sleep(Duration::from_millis(10));

            // 检查是否所有线程都完成了
            let all_finished = handles.iter().all(|h| h.is_finished());
            if all_finished {
                break;
            }
        }

        // 收集所有线程的结果
        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                solution = Some(result);
                break;
            }
        }

        solution
    }
}

// 计算哈希函数
pub fn calculate_hash(nonce: U256, address: Address, solution: U256) -> [u8; 32] {
    // 编码参数
    let mut encoded = Vec::with_capacity(32 + 20 + 32);

    // 添加nonce (32字节)
    let mut nonce_bytes = [0u8; 32];
    nonce.to_big_endian(&mut nonce_bytes);
    encoded.extend_from_slice(&nonce_bytes);

    // 添加地址 (20字节)
    encoded.extend_from_slice(address.as_bytes());

    // 添加解决方案 (32字节)
    let mut solution_bytes = [0u8; 32];
    solution.to_big_endian(&mut solution_bytes);
    encoded.extend_from_slice(&solution_bytes);

    // 计算keccak256哈希
    keccak256(encoded)
}

// 验证解决方案
pub fn verify_solution(nonce: U256, address: Address, solution: U256, difficulty: U256) -> bool {
    // 计算哈希
    let hash = calculate_hash(nonce, address, solution);

    // 转换为BigUint进行比较
    let hash_bigint = BigUint::from_bytes_be(&hash);
    let difficulty_bigint = BigUint::from_str_radix(&difficulty.to_string(), 10).unwrap();

    // 计算目标值: 2^256 / difficulty
    let base: BigUint = BigUint::from(1u32);
    let shifted: BigUint = base.shl(256);
    let target = shifted / difficulty_bigint;

    // 检查哈希是否小于目标值
    hash_bigint <= target
}

// 快速验证解决方案 (使用U256，避免BigUint转换开销)
pub fn fast_verify_solution(
    nonce: U256,
    address: Address,
    solution: U256,
    difficulty: U256,
) -> bool {
    // 计算哈希
    let hash = calculate_hash(nonce, address, solution);

    // 转换为U256
    let hash_u256 = U256::from_big_endian(&hash);

    // 检查 hash * difficulty < 2^256
    // 等价于 hash < 2^256 / difficulty
    let max_u256 = !U256::zero(); // 2^256 - 1

    // 使用比较方法1: hash * difficulty < 2^256
    if let Some(product) = hash_u256.checked_mul(difficulty) {
        if product <= max_u256 {
            return true;
        }
    }

    // 使用比较方法2: 手动比较前导零
    // 对于高难度值，哈希应该有足够多的前导零
    let leading_zeros = hash.iter().take_while(|&&b| b == 0).count();
    let expected_zeros = (difficulty.bits() as usize).saturating_sub(256) / 8;

    leading_zeros >= expected_zeros
}
