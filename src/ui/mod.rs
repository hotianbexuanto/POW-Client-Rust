pub mod app;
pub mod event;
pub mod ui;

pub use app::{
    App, AppConfig, AppState, LogLevel, MiningSessionInfo, MiningStatus, RpcNodeStatus, TaskInfo,
    TaskTimingStats,
};
pub use event::{Event, EventHandler};
pub use ui::render;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    io::{self, Stdout},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

// 定义TUI相关类型
pub type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;
pub type EventSender = mpsc::Sender<Event<()>>;
pub type EventReceiver = mpsc::Receiver<Event<()>>;

// TUI初始化函数
pub fn init_tui() -> Result<(TuiTerminal, EventSender, EventReceiver)> {
    // 设置终端
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    // 设置事件处理
    let (tx, rx) = mpsc::channel(100);
    let event_handler = EventHandler::new(TICK_RATE);
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        event_handler.start(tx_clone).await;
    });

    Ok((terminal, tx, rx))
}

// TUI销毁函数
pub fn destroy_tui(terminal: &mut TuiTerminal) -> Result<()> {
    // 恢复终端
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

// 创建共享的应用状态
pub fn create_app() -> Arc<Mutex<App>> {
    Arc::new(Mutex::new(App::new()))
}

// 定义tick_rate常量
const TICK_RATE: Duration = Duration::from_secs(1);
