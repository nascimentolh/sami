use std::{collections::HashMap, sync::Arc};

use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, Notifier},
    grid::Dimensions,
    sync::FairMutex,
    term::{Config, Term, TermMode, cell::Flags},
    tty,
    vte::ansi::Rgb,
};
use futures::{StreamExt, channel::mpsc};
use gpui_kit::*;

use crate::{keys, palette::Palette};

const FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
const FONT_PT: f32 = 13.0;
const PADDING: f32 = 8.0;

#[derive(Clone)]
struct Listener(mpsc::UnboundedSender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

#[derive(Clone, Copy, PartialEq)]
struct GridSize {
    cols: usize,
    lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    focus: FocusHandle,
    palette: Palette,
    grid: GridSize,
    _events: Task<()>,
}

struct Segment {
    col: usize,
    text: String,
    runs: Vec<TextRun>,
}

struct Frame {
    rows: Vec<Vec<Segment>>,
    backgrounds: Vec<(usize, usize, usize, Hsla)>,
}

fn hsla(c: Rgb) -> Hsla {
    rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded();
        let listener = Listener(tx);
        let grid = GridSize {
            cols: 80,
            lines: 24,
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &grid, listener.clone())));

        let env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);
        let options = tty::Options {
            shell: None,
            working_directory: None,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&options, window_size(grid, px(0.), px(0.)), 0).expect("abrir o PTY");
        let event_loop =
            EventLoop::new(term.clone(), listener, pty, true, false).expect("criar o loop do PTY");
        let pty = Notifier(event_loop.channel());
        event_loop.spawn();

        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| this.on_term_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            term,
            pty,
            focus: cx.focus_handle(),
            palette: Palette::default(),
            grid,
            _events: events,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn on_term_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup => cx.notify(),
            TermEvent::PtyWrite(text) => self.pty.notify(text.into_bytes()),
            TermEvent::Exit | TermEvent::ChildExit(_) => cx.quit(),
            _ => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::to_bytes(&event.keystroke, app_cursor) {
            self.pty.notify(bytes);
            cx.stop_propagation();
        }
    }

    fn fit(&mut self, viewport: Size<Pixels>, cell: Size<Pixels>) {
        let cols = (((viewport.width - px(2. * PADDING)) / cell.width).floor() as usize).max(2);
        let lines = (((viewport.height - px(2. * PADDING)) / cell.height).floor() as usize).max(1);
        let grid = GridSize { cols, lines };
        if grid != self.grid {
            self.grid = grid;
            self.term.lock().resize(grid);
            self.pty
                .on_resize(window_size(grid, cell.width, cell.height));
        }
    }

    fn snapshot(&self, font: &Font) -> Frame {
        let term = self.term.lock();
        let content = term.renderable_content();
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        let offset = content.display_offset as i32;
        let cursor = (
            content.cursor.point.line.0 + offset,
            content.cursor.point.column.0,
        );
        let default_bg = self.palette.background;

        let mut rows: Vec<Vec<Segment>> = (0..self.grid.lines).map(|_| Vec::new()).collect();
        let mut backgrounds = Vec::new();

        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if row < 0 || row as usize >= rows.len() {
                continue;
            }

            let mut fg = self.palette.resolve(cell.fg, content.colors);
            let mut bg = self.palette.resolve(cell.bg, content.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if show_cursor && (row, col) == cursor {
                fg = self.palette.cursor_text;
                bg = self.palette.cursor;
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            if bg != default_bg {
                backgrounds.push((row as usize, col, width, hsla(bg)));
            }

            let mut text = String::from(cell.c);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            let mut run_font = font.clone();
            if cell.flags.contains(Flags::BOLD) {
                run_font = run_font.bold();
            }
            if cell.flags.contains(Flags::ITALIC) {
                run_font = run_font.italic();
            }
            let underline = cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(hsla(fg)),
                    wavy: false,
                });
            let run = TextRun {
                len: text.len(),
                font: run_font,
                color: hsla(fg),
                background_color: None,
                underline,
                strikethrough: None,
            };

            // ASCII contíguo vira um segmento só (ligaduras); o resto vai na coluna exata, porque glifo
            // largo de Nerd Font ou CJK quebraria o alinhamento do que vem depois
            let line = &mut rows[row as usize];
            let ascii = text.is_ascii();
            match line.last_mut() {
                Some(seg) if ascii && seg.text.is_ascii() && seg.col + seg.text.len() == col => {
                    seg.text.push_str(&text);
                    match seg.runs.last_mut() {
                        Some(last)
                            if last.font == run.font
                                && last.color == run.color
                                && last.underline == run.underline =>
                        {
                            last.len += run.len
                        }
                        _ => seg.runs.push(run),
                    }
                }
                _ => line.push(Segment {
                    col,
                    text,
                    runs: vec![run],
                }),
            }
        }
        Frame { rows, backgrounds }
    }
}

fn window_size(grid: GridSize, cell_width: Pixels, cell_height: Pixels) -> WindowSize {
    WindowSize {
        num_lines: grid.lines as u16,
        num_cols: grid.cols as u16,
        cell_width: f32::from(cell_width) as u16,
        cell_height: f32::from(cell_height) as u16,
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = font(FONT_FAMILY);
        let font_size = px(FONT_PT * 96. / 72.);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font);
        let cell = Size {
            width: text_system
                .advance(font_id, font_size, 'm')
                .map(|s| s.width)
                .unwrap_or(font_size * 0.6),
            height: (text_system.ascent(font_id, font_size)
                + text_system.descent(font_id, font_size).abs())
            .ceil(),
        };
        self.fit(window.viewport_size(), cell);
        let frame = self.snapshot(&font);
        let bg = hsla(self.palette.background);

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(bg)
            .p(px(PADDING))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let origin = bounds.origin;
                        for (row, col, width, color) in frame.backgrounds {
                            let at = point(
                                origin.x + cell.width * col as f32,
                                origin.y + cell.height * row as f32,
                            );
                            window.paint_quad(fill(
                                Bounds::new(at, size(cell.width * width as f32, cell.height)),
                                color,
                            ));
                        }
                        for (row, segments) in frame.rows.into_iter().enumerate() {
                            for seg in segments {
                                let at = point(
                                    origin.x + cell.width * seg.col as f32,
                                    origin.y + cell.height * row as f32,
                                );
                                let shaped = window.text_system().shape_line(
                                    seg.text.into(),
                                    font_size,
                                    &seg.runs,
                                    None,
                                );
                                let _ = shaped.paint(
                                    at,
                                    cell.height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}
