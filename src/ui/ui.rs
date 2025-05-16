use crate::ui::app::{App, LogLevel};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame,
};
use std::time::Duration;

// 主渲染函数
pub fn render<B: Backend>(f: &mut Frame, app: &App) {
    // 创建主布局，分为上下两部分
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(60), // 上部分占60%
            Constraint::Percentage(40), // 下部分占40%
        ])
        .split(f.size());

    // 渲染上半部分信息区域
    render_info_area::<B>(f, app, chunks[0]);

    // 渲染下半部分日志区域
    render_logs_area::<B>(f, app, chunks[1]);
}

// 渲染信息区域
fn render_info_area<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 将信息区域分为左右两部分
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // 左侧占30%
            Constraint::Percentage(70), // 右侧占70%
        ])
        .split(area);

    // 渲染左侧钱包和合约信息
    render_wallet_info::<B>(f, app, chunks[0]);

    // 将右侧区域再分为上下两部分
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40), // 上部分占40%
            Constraint::Percentage(60), // 下部分占60%
        ])
        .split(chunks[1]);

    // 渲染右上方挖矿摘要信息
    render_mining_summary::<B>(f, app, right_chunks[0]);

    // 渲染右下方任务列表
    render_task_list::<B>(f, app, right_chunks[1]);
}

// 渲染钱包和合约信息
fn render_wallet_info<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 分割区域为上下两部分
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50), // 上部分占50%
            Constraint::Percentage(50), // 下部分占50%
        ])
        .split(area);

    // 钱包信息
    let wallet_info = vec![
        Line::from(vec![
            Span::styled("钱包地址: ", Style::default().fg(Color::Cyan)),
            Span::raw(match &app.wallet_address {
                Some(addr) => format!("{:?}", addr),
                None => "未连接".to_string(),
            }),
        ]),
        Line::from(vec![
            Span::styled("钱包余额: ", Style::default().fg(Color::Cyan)),
            Span::raw(match app.wallet_balance {
                Some(balance) => format!("{:.4} MAG", balance),
                None => "未知".to_string(),
            }),
        ]),
    ];

    // 合约信息
    let contract_info = vec![
        Line::from(vec![
            Span::styled("合约地址: ", Style::default().fg(Color::Cyan)),
            Span::raw(match &app.contract_address {
                Some(addr) => format!("{:?}", addr),
                None => "未连接".to_string(),
            }),
        ]),
        Line::from(vec![
            Span::styled("合约余额: ", Style::default().fg(Color::Cyan)),
            Span::raw(match app.contract_balance {
                Some(balance) => format!("{:.4} MAG", balance),
                None => "未知".to_string(),
            }),
        ]),
    ];

    // 渲染钱包信息
    let wallet_block = Block::default().borders(Borders::ALL).title("钱包信息");
    let wallet_paragraph = Paragraph::new(Text::from(wallet_info))
        .block(wallet_block)
        .wrap(Wrap { trim: true });
    f.render_widget(wallet_paragraph, chunks[0]);

    // 渲染合约信息
    let contract_block = Block::default().borders(Borders::ALL).title("合约信息");
    let contract_paragraph = Paragraph::new(Text::from(contract_info))
        .block(contract_block)
        .wrap(Wrap { trim: true });
    f.render_widget(contract_paragraph, chunks[1]);
}

// 渲染挖矿摘要信息
fn render_mining_summary<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 格式化运行时间
    let uptime_str = format_duration(Duration::from_secs(app.mining_status.uptime));

    // 挖矿摘要信息
    let mining_summary = vec![
        Line::from(vec![
            Span::styled("活跃任务: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!(
                "{}/{}",
                app.mining_status.active_tasks, app.mining_status.total_tasks
            )),
        ]),
        Line::from(vec![
            Span::styled("总哈希率: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{:.2} H/s", app.mining_status.total_hash_rate)),
        ]),
        Line::from(vec![
            Span::styled("找到解决方案: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.mining_status.total_solutions_found)),
        ]),
        Line::from(vec![
            Span::styled("运行时间: ", Style::default().fg(Color::Yellow)),
            Span::raw(uptime_str),
        ]),
    ];

    // 渲染挖矿摘要
    let mining_block = Block::default().borders(Borders::ALL).title("挖矿摘要");
    let mining_paragraph = Paragraph::new(Text::from(mining_summary))
        .block(mining_block)
        .wrap(Wrap { trim: true });
    f.render_widget(mining_paragraph, area);
}

// 渲染任务列表
fn render_task_list<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 表头
    let header_cells = ["ID", "Nonce", "难度", "状态", "哈希率"].iter().map(|h| {
        Span::styled(
            *h,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).style(Style::default().fg(Color::Yellow));

    // 任务行
    let rows = app.tasks.iter().map(|task| {
        let id = task.id.to_string();
        let nonce = match task.nonce {
            Some(n) => format!("{:?}", n),
            None => "-".to_string(),
        };
        let difficulty = match task.difficulty {
            Some(d) => format!("{:?}", d),
            None => "-".to_string(),
        };
        let status = task.status.clone();
        let hash_rate = match task.hash_rate {
            Some(rate) => format!("{:.2} H/s", rate),
            None => "-".to_string(),
        };

        let status_style = match status.as_str() {
            "成功" => Style::default().fg(Color::Green),
            "处理中" => Style::default().fg(Color::Yellow),
            "错误" => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::White),
        };

        let cells = vec![
            Span::raw(id),
            Span::raw(nonce),
            Span::raw(difficulty),
            Span::styled(status, status_style),
            Span::raw(hash_rate),
        ];
        Row::new(cells)
    });

    // 创建任务表
    let widths = [
        Constraint::Percentage(10), // ID
        Constraint::Percentage(25), // Nonce
        Constraint::Percentage(25), // 难度
        Constraint::Percentage(20), // 状态
        Constraint::Percentage(20), // 哈希率
    ];

    let task_table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("任务列表"));

    f.render_widget(task_table, area);
}

// 渲染日志区域
fn render_logs_area<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 创建日志列表项
    let log_items: Vec<ListItem> = app
        .logs
        .iter()
        .map(|log| {
            // 根据日志级别设置样式
            let style = match log.level {
                LogLevel::Info => Style::default().fg(Color::White),
                LogLevel::Success => Style::default().fg(Color::Green),
                LogLevel::Warning => Style::default().fg(Color::Yellow),
                LogLevel::Error => Style::default().fg(Color::Red),
            };

            // 格式化时间戳
            let time = log.timestamp.format("%H:%M:%S").to_string();

            // 创建带有样式的日志项
            let content = Line::from(vec![
                Span::styled(format!("[{}] ", time), Style::default().fg(Color::Blue)),
                Span::styled(&log.message, style),
            ]);

            ListItem::new(content)
        })
        .collect();

    // 创建日志列表
    let logs = List::new(log_items)
        .block(Block::default().borders(Borders::ALL).title("日志"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(">> ");

    f.render_widget(logs, area);
}

// 格式化持续时间为可读格式
fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    format!("{}:{:02}:{:02}", hours, minutes, seconds)
}
