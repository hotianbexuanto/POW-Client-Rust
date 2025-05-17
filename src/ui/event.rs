use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyModifiers};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc as tokio_mpsc;

// 自定义事件类型
#[derive(Debug, Clone, Copy)]
pub enum Event<I> {
    Input(I),
    Tick,
}

/// 事件处理器，处理键盘输入和定时tick
pub struct EventHandler {
    // 定时tick速率
    tick_rate: Duration,
}

impl EventHandler {
    pub fn new(tick_rate: Duration) -> Self {
        Self { tick_rate }
    }

    /// 启动事件循环
    pub async fn start(&self, sender: tokio_mpsc::Sender<Event<()>>) {
        let tick_rate = self.tick_rate;
        let (tx, rx) = mpsc::channel();

        // 在单独的线程中处理输入事件
        thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                // 计算下一个tick的时间
                let timeout = tick_rate
                    .checked_sub(last_tick.elapsed())
                    .unwrap_or_else(|| Duration::from_secs(0));

                // 等待用户输入，最多等待到下一个tick
                if event::poll(timeout).expect("事件轮询出错") {
                    if let CrosstermEvent::Key(key) = event::read().expect("读取事件出错") {
                        // 处理退出快捷键 (Ctrl+C 或 Ctrl+Q 或 'q')
                        if (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('q'))
                            && key.modifiers == KeyModifiers::CONTROL
                            || key.code == KeyCode::Char('q')
                        {
                            tx.send(Event::Input(())).expect("无法发送输入事件");
                        }
                    }
                }

                // 处理定时tick
                if last_tick.elapsed() >= tick_rate {
                    tx.send(Event::Tick).expect("无法发送定时事件");
                    last_tick = Instant::now();
                }
            }
        });

        // 将消息从标准mpsc转发到tokio mpsc
        loop {
            if let Ok(event) = rx.recv() {
                sender.send(event).await.expect("无法发送事件");

                // 当收到用户输入事件时，退出循环
                if let Event::Input(_) = event {
                    break;
                }
            }
        }
    }
}
