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

    // 渲染左侧钱包、合约和配置信息
    render_left_panel::<B>(f, app, chunks[0]);

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

// 渲染左侧面板（钱包、合约、配置信息）
fn render_left_panel<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 分割区域为上中下三部分
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30), // 上部分占30%
            Constraint::Percentage(30), // 中部分占30%
            Constraint::Percentage(40), // 下部分占40%
        ])
        .split(area);

    // 渲染钱包信息
    render_wallet_info::<B>(f, app, chunks[0]);

    // 渲染合约信息
    render_contract_info::<B>(f, app, chunks[1]);

    // 渲染配置信息
    render_config_info::<B>(f, app, chunks[2]);
}

// 渲染钱包信息
fn render_wallet_info<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
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
        Line::from(vec![
            Span::styled("挖矿获得: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.2} MAG", app.mining_status.total_mined)),
        ]),
    ];

    // 渲染钱包信息
    let wallet_block = Block::default().borders(Borders::ALL).title("钱包信息");
    let wallet_paragraph = Paragraph::new(Text::from(wallet_info))
        .block(wallet_block)
        .wrap(Wrap { trim: true });
    f.render_widget(wallet_paragraph, area);
}

// 渲染合约信息
fn render_contract_info<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
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

    // 渲染合约信息
    let contract_block = Block::default().borders(Borders::ALL).title("合约信息");
    let contract_paragraph = Paragraph::new(Text::from(contract_info))
        .block(contract_block)
        .wrap(Wrap { trim: true });
    f.render_widget(contract_paragraph, area);
}

// 渲染配置信息
fn render_config_info<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 配置信息
    let config_info = vec![
        Line::from(vec![
            Span::styled("任务数量: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.config.task_count)),
        ]),
        Line::from(vec![
            Span::styled("每任务线程数: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.config.thread_count)),
        ]),
        Line::from(vec![
            Span::styled("自动选择RPC: ", Style::default().fg(Color::Yellow)),
            Span::raw(if app.config.auto_select_rpc {
                "是"
            } else {
                "否"
            }),
        ]),
        Line::from(vec![
            Span::styled("自动滚动日志: ", Style::default().fg(Color::Yellow)),
            Span::raw(if app.config.auto_scroll_logs {
                "是"
            } else {
                "否"
            }),
        ]),
        Line::from(vec![
            Span::styled("RPC节点: ", Style::default().fg(Color::Yellow)),
            Span::raw(match &app.current_rpc {
                Some(rpc) => rpc.clone(),
                None => "未选择".to_string(),
            }),
        ]),
    ];

    // 渲染配置信息
    let config_block = Block::default().borders(Borders::ALL).title("配置信息");
    let config_paragraph = Paragraph::new(Text::from(config_info))
        .block(config_block)
        .wrap(Wrap { trim: true });
    f.render_widget(config_paragraph, area);
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
            Span::styled("挖矿获得: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.2} MAG", app.mining_status.total_mined)),
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

    // 渲染任务表格
    let widths = vec![
        Constraint::Percentage(10),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(20),
        Constraint::Percentage(20),
    ];
    let task_table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("挖矿任务"));
    f.render_widget(task_table, area);
}

// 渲染日志区域
fn render_logs_area<B: Backend>(f: &mut Frame, app: &App, area: Rect) {
    // 日志块
    let log_block = Block::default().borders(Borders::ALL).title("日志信息");
    f.render_widget(log_block.clone(), area);

    // 获取内部区域
    let inner_area = log_block.inner(area);

    // 计算可以显示的日志行数
    let max_logs_visible = inner_area.height as usize;

    // 处理日志滚动
    let logs_to_show = if app.logs.len() > max_logs_visible {
        let start_idx = app.log_scroll.min(app.logs.len() - max_logs_visible);
        app.logs.len() - max_logs_visible - start_idx
    } else {
        0
    };

    // 创建日志项
    let log_items: Vec<ListItem> = app
        .logs
        .iter()
        .skip(logs_to_show)
        .take(max_logs_visible)
        .map(|log| {
            let time = log.timestamp.format("%H:%M:%S").to_string();
            let level_style = match log.level {
                LogLevel::Info => Style::default().fg(Color::White),
                LogLevel::Success => Style::default().fg(Color::Green),
                LogLevel::Warning => Style::default().fg(Color::Yellow),
                LogLevel::Error => Style::default().fg(Color::Red),
            };

            let log_line = Line::from(vec![
                Span::styled(format!("[{}] ", time), Style::default().fg(Color::Gray)),
                Span::styled(log.message.clone(), level_style),
            ]);

            ListItem::new(log_line)
        })
        .collect();

    // 渲染日志列表
    let logs = List::new(log_items)
        .block(Block::default())
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(logs, inner_area);

    // 显示滚动指示器（如果需要滚动）
    if app.logs.len() > max_logs_visible {
        let scroll_info = format!(
            "[滚动位置: {}/{}]",
            app.log_scroll,
            app.logs.len() - max_logs_visible
        );
        let scroll_text = Paragraph::new(scroll_info)
            .style(Style::default().fg(Color::Gray))
            .alignment(ratatui::layout::Alignment::Right);

        // 计算滚动信息的位置
        let scroll_area = Rect::new(inner_area.x + inner_area.width - 20, inner_area.y, 20, 1);

        f.render_widget(scroll_text, scroll_area);
    }
}

// 格式化持续时间
fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}
