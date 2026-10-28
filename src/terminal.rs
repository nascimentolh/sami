use std::{collections::HashMap, ops::Range, sync::Arc};

use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, Notifier},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::ClipboardType,
    term::{Config, Term, TermMode, cell::Flags},
    tty,
    vte::ansi::{Color, NamedColor, Rgb},
};
use futures::{StreamExt, channel::mpsc};
use gpui_kit::*;

use crate::{binds::Binds, braille, ghostty::GhosttyConfig, keys, mouse, palette::Palette};

#[derive(Clone)]
struct Listener(mpsc::UnboundedSender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

/// A área PRIMARY (copy-on-select, colar com o botão do meio) é do X11/Wayland: no Windows e no mac não existe.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn write_primary(cx: &mut Context<TerminalView>, text: String) {
    cx.write_to_primary(ClipboardItem::new_string(text));
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn write_primary(_: &mut Context<TerminalView>, _: String) {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn read_primary(cx: &mut Context<TerminalView>) -> Option<String> {
    cx.read_from_primary().and_then(|i| i.text())
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn read_primary(_: &mut Context<TerminalView>) -> Option<String> {
    None
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
    font: Font,
    font_size: Pixels,
    padding: Point<Pixels>,
    /// canto da grade na janela: os eventos do mouse vêm em coordenadas da janela
    origin: Point<Pixels>,
    opacity: f32,
    grid: GridSize,
    cell: Size<Pixels>,
    scroll_rem: f32,
    last_motion: Option<GridPoint>,
    /// linhas do topo que o app desenha mas o Maestro esconde (a barra de abas do herdr)
    hidden_top: usize,
    prefix_pending: bool,
    binds: Binds,
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
    hollow_cursor: Option<(usize, usize)>,
    braille: Vec<(usize, usize, char, Hsla)>,
}

// o Root do gpui-kit liga Tab à navegação de foco; no terminal o Tab é do shell (autocomplete)
actions!(terminal, [SendTab, SendBackTab]);

/// O que rodar no PTY no lugar do `$SHELL`.
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub hidden_top: usize,
}

pub fn hsla(c: Rgb) -> Hsla {
    rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

impl TerminalView {
    pub fn new(
        config: GhosttyConfig,
        launch: Option<Launch>,
        binds: Binds,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded();
        let listener = Listener(tx);
        let grid = GridSize {
            cols: 80,
            lines: 24,
        };
        let term_config = Config {
            scrolling_history: 10_000,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            term_config,
            &grid,
            listener.clone(),
        )));

        let mut env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);
        if let Some(l) = &launch {
            env.extend(l.env.clone());
        }
        let options = tty::Options {
            shell: launch
                .as_ref()
                .map(|l| tty::Shell::new(l.program.clone(), l.args.clone())),
            working_directory: None,
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
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
            palette: config.palette,
            font: Font {
                features: FontFeatures(Arc::new(config.font_features)),
                ..font(config.font_family)
            },
            // ponytail: 96 DPI fixo (conferido com o Ghostty); ler o DPI do monitor quando houver tela HiDPI
            font_size: px(config.font_size_pt * 96. / 72.),
            padding: point(px(config.padding_x), px(config.padding_y)),
            origin: point(px(config.padding_x), px(config.padding_y)),
            opacity: config.background_opacity,
            grid,
            cell: size(px(10.), px(20.)),
            scroll_rem: 0.,
            last_motion: None,
            hidden_top: launch.as_ref().map_or(0, |l| l.hidden_top),
            prefix_pending: false,
            binds,
            _events: events,
        }
    }

    /// Ação do herdr pedida pela paleta, como se a tecla fosse digitada aqui.
    pub fn herdr_action(&mut self, name: &str) {
        if let Some(bytes) = self.binds.action(name) {
            self.input(bytes.to_vec());
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.input(bytes.to_vec());
    }

    /// O último byte enviado foi o prefixo do herdr: a barra mostra o modo PREFIX até a próxima tecla.
    pub fn prefix_pending(&self) -> bool {
        self.prefix_pending
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn on_term_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup => cx.notify(),
            TermEvent::PtyWrite(text) => self.pty.notify(text.into_bytes()),
            TermEvent::Exit | TermEvent::ChildExit(_) => cx.quit(),
            // OSC 10/11/4: herdr e vim perguntam as cores para se adaptar ao tema
            TermEvent::ColorRequest(i, reply) => {
                let color = match i {
                    i if i < 256 => Color::Indexed(i as u8),
                    257 => Color::Named(NamedColor::Background),
                    258 => Color::Named(NamedColor::Cursor),
                    _ => Color::Named(NamedColor::Foreground),
                };
                let rgb = self.palette.resolve(color, self.term.lock().colors());
                self.pty.notify(reply(rgb).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(reply) => {
                let size = window_size(self.grid, self.cell.width, self.cell.height);
                self.pty.notify(reply(size).into_bytes());
            }
            // OSC 52 só de escrita; ler o clipboard pelo terminal vazaria dados para qualquer programa
            TermEvent::ClipboardStore(ClipboardType::Clipboard, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardStore(ClipboardType::Selection, text) => write_primary(cx, text),
            _ => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        // atalhos do próprio terminal, iguais aos defaults do Ghostty no Linux
        let handled = match (m.control, m.shift, k.key.as_str()) {
            (true, true, "c") => {
                if let Some(text) = self.term.lock().selection_to_string() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                true
            }
            (true, true, "v") | (false, true, "insert") => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.paste(&text);
                }
                true
            }
            (false, true, "pageup") => self.scroll(Scroll::PageUp, cx),
            (false, true, "pagedown") => self.scroll(Scroll::PageDown, cx),
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            return;
        }
        if let Some(bytes) = self.binds.get(k) {
            self.input(bytes.to_vec());
            cx.stop_propagation();
            return;
        }
        if keys::is_app_shortcut(k) {
            return;
        }
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::to_bytes(k, app_cursor) {
            self.input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn input(&mut self, bytes: Vec<u8>) {
        self.prefix_pending = self.binds.prefix.as_ref() == Some(&bytes);
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        drop(term);
        self.pty.notify(bytes);
    }

    fn paste(&mut self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            // sem ESC dentro do bracket, senão o texto colado consegue fechar o bracket e virar comando
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes);
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) -> bool {
        self.term.lock().scroll_display(scroll);
        cx.notify();
        true
    }

    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let x = (f32::from(position.x - self.origin.x)).max(0.) / f32::from(self.cell.width);
        let y = (f32::from(position.y - self.origin.y)).max(0.) / f32::from(self.cell.height);
        let col = (x as usize).min(self.grid.cols - 1);
        let line = (y as usize + self.hidden_top).min(self.grid.lines - 1);
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = self.term.lock().grid().display_offset() as i32;
        (
            GridPoint::new(Line(line as i32 - offset), Column(col)),
            side,
        )
    }

    fn mouse_report(&self, shift: bool) -> Option<bool> {
        let mode = *self.term.lock().mode();
        (mode.intersects(TermMode::MOUSE_MODE) && !shift)
            .then(|| mode.contains(TermMode::SGR_MOUSE))
    }

    fn send_mouse(
        &mut self,
        button: u8,
        drag: bool,
        pressed: bool,
        position: Point<Pixels>,
        mods: &Modifiers,
        sgr: bool,
    ) {
        let (p, _) = self.grid_point(position);
        let line = (p.line.0 + self.term.lock().grid().display_offset() as i32).max(0) as usize;
        if let Some(bytes) = mouse::report(button, drag, pressed, p.column.0, line, mods, sgr) {
            self.pty.notify(bytes);
        }
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let button = match e.button {
            MouseButton::Left => mouse::LEFT,
            MouseButton::Middle => mouse::MIDDLE,
            MouseButton::Right => mouse::RIGHT,
            _ => return,
        };
        if let Some(sgr) = self.mouse_report(e.modifiers.shift) {
            self.send_mouse(button, false, true, e.position, &e.modifiers, sgr);
            return;
        }
        match e.button {
            MouseButton::Left => {
                let (point, side) = self.grid_point(e.position);
                let ty = match e.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                self.term.lock().selection = Some(Selection::new(ty, point, side));
                cx.notify();
            }
            MouseButton::Middle => {
                if let Some(text) = read_primary(cx) {
                    self.paste(&text);
                }
            }
            _ => {}
        }
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sgr) = self.mouse_report(e.modifiers.shift) {
            let mode = *self.term.lock().mode();
            let button = match e.pressed_button {
                Some(MouseButton::Left) => Some(mouse::LEFT),
                Some(MouseButton::Middle) => Some(mouse::MIDDLE),
                Some(MouseButton::Right) => Some(mouse::RIGHT),
                _ => None,
            };
            // 1002 reporta só arrasto; 1003 (herdr) também o movimento sem botão
            let report = match button {
                Some(b) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                    Some(b)
                }
                None if mode.contains(TermMode::MOUSE_MOTION) => Some(mouse::NO_BUTTON),
                _ => None,
            };
            let cell = self.grid_point(e.position).0;
            if let Some(b) = report
                && self.last_motion != Some(cell)
            {
                self.last_motion = Some(cell);
                self.send_mouse(b, true, true, e.position, &e.modifiers, sgr);
            }
            return;
        }
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (point, side) = self.grid_point(e.position);
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(point, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sgr) = self.mouse_report(e.modifiers.shift) {
            let button = match e.button {
                MouseButton::Left => mouse::LEFT,
                MouseButton::Middle => mouse::MIDDLE,
                MouseButton::Right => mouse::RIGHT,
                _ => return,
            };
            self.send_mouse(button, false, false, e.position, &e.modifiers, sgr);
            return;
        }
        // copy-on-select do Ghostty no Linux: a seleção vai para a área PRIMARY (colar com o botão do meio)
        let selected = self.term.lock().selection_to_string();
        if e.button == MouseButton::Left
            && let Some(text) = selected.filter(|t| !t.is_empty())
        {
            write_primary(cx, text);
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = self.scroll_rem
            + f32::from(e.delta.pixel_delta(self.cell.height).y) / f32::from(self.cell.height);
        self.scroll_rem = lines.fract();
        let n = lines.trunc() as i32;
        if n == 0 {
            return;
        }
        let mode = *self.term.lock().mode();
        if let Some(sgr) = self.mouse_report(e.modifiers.shift) {
            let button = if n > 0 {
                mouse::WHEEL_UP
            } else {
                mouse::WHEEL_DOWN
            };
            for _ in 0..n.unsigned_abs() {
                self.send_mouse(button, false, true, e.position, &e.modifiers, sgr);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let arrow: &[u8] = if n > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.pty.notify(arrow.repeat(n.unsigned_abs() as usize));
        } else {
            self.scroll(Scroll::Delta(n), cx);
        }
    }

    /// `area` é o espaço do canvas, já sem o padding.
    fn fit(&mut self, area: Size<Pixels>, cell: Size<Pixels>, cx: &mut Context<Self>) {
        let cols = ((area.width / cell.width).floor() as usize).max(2);
        let lines = ((area.height / cell.height).floor() as usize).max(1) + self.hidden_top;
        let grid = GridSize { cols, lines };
        if grid != self.grid {
            self.grid = grid;
            self.term.lock().resize(grid);
            self.pty
                .on_resize(window_size(grid, cell.width, cell.height));
            cx.notify();
        }
    }

    fn snapshot(&self, font: &Font, focused: bool) -> Frame {
        let term = self.term.lock();
        let content = term.renderable_content();
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        let offset = content.display_offset as i32 - self.hidden_top as i32;
        let selection = content.selection;
        let cursor = (
            content.cursor.point.line.0 + offset,
            content.cursor.point.column.0,
        );
        let default_bg = self.palette.background;

        let mut rows: Vec<Vec<Segment>> = (self.hidden_top..self.grid.lines)
            .map(|_| Vec::new())
            .collect();
        let mut backgrounds = Vec::new();
        let mut hollow_cursor = None;
        let mut braille = Vec::new();

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
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                fg = self.palette.selection_fg;
                bg = self.palette.selection_bg;
            }
            if show_cursor && (row, col) == cursor {
                if focused {
                    fg = self.palette.cursor_text;
                    bg = self.palette.cursor;
                } else {
                    hollow_cursor = Some((row as usize, col));
                }
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

            if cell.zerowidth().is_none() && ('\u{2800}'..='\u{28ff}').contains(&cell.c) {
                braille.push((row as usize, col, cell.c, hsla(fg)));
                continue;
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
        Frame {
            rows,
            backgrounds,
            hollow_cursor,
            braille,
        }
    }
}

/// Glifo a glifo na coluna dele: o avanço natural da fonte (10.4px) desalinharia da grade inteira.
/// A ligadura sobrevive porque a JetBrains Mono troca glifos sem mudar a quantidade.
fn paint_on_grid(
    shaped: &ShapedLine,
    runs: &[TextRun],
    at: Point<Pixels>,
    cell: Size<Pixels>,
    font_size: Pixels,
    window: &mut Window,
) {
    let baseline = at.y + (cell.height - shaped.ascent - shaped.descent) / 2. + shaped.ascent;
    let mut start = 0;
    let spans: Vec<_> = runs
        .iter()
        .map(|r| {
            start += r.len;
            (start - r.len..start, r)
        })
        .collect();
    for run in &shaped.runs {
        for glyph in &run.glyphs {
            let Some((_, style)) = spans.iter().find(|(r, _)| r.contains(&glyph.index)) else {
                continue;
            };
            let origin = point(at.x + cell.width * glyph.index as f32, baseline);
            let _ = if glyph.is_emoji {
                window.paint_emoji(origin, run.font_id, glyph.id, font_size)
            } else {
                window.paint_glyph(origin, run.font_id, glyph.id, font_size, style.color)
            };
        }
    }
    for (range, style) in &spans {
        if let Some(u) = &style.underline {
            let y = baseline + px(2.);
            window.paint_quad(fill(
                Bounds::new(
                    point(at.x + cell.width * range.start as f32, y),
                    size(cell.width * range.len() as f32, u.thickness),
                ),
                u.color.unwrap_or(style.color),
            ));
        }
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
        let font = self.font.clone();
        let font_size = self.font_size;
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font);
        // grade em pixels inteiros, como o Ghostty (13pt: 10.4x22.9 vira 10x22); o glifo fica no tamanho real
        let cell = Size {
            width: text_system
                .advance(font_id, font_size, 'm')
                .map(|s| s.width)
                .unwrap_or(font_size * 0.6)
                .floor(),
            height: (text_system.ascent(font_id, font_size)
                + text_system.descent(font_id, font_size).abs())
            .floor(),
        };
        self.cell = cell;
        let entity = cx.entity();
        let focus = self.focus.clone();
        let focused = self.focus.is_focused(window) && window.is_window_active();
        let frame = self.snapshot(&font, focused);
        let cursor_color = hsla(self.palette.cursor);
        let bg = Hsla {
            a: self.opacity,
            ..hsla(self.palette.background)
        };

        div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_action(cx.listener(|this, _: &SendTab, _, cx| {
                this.input(b"\t".to_vec());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SendBackTab, _, cx| {
                this.input(b"\x1b[Z".to_vec());
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .size_full()
            .bg(bg)
            .px(self.padding.x)
            .py(self.padding.y)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        // a grade segue o espaço real do terminal (a lateral ocupa parte da janela)
                        entity.update(cx, |this, cx| {
                            this.origin = bounds.origin;
                            this.fit(bounds.size, cell, cx)
                        });
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
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
                        for (row, col, c, color) in frame.braille {
                            let at = point(
                                origin.x + cell.width * col as f32,
                                origin.y + cell.height * row as f32,
                            );
                            let dots = braille::dots(
                                c,
                                f32::from(cell.width) as u32,
                                f32::from(cell.height) as u32,
                            );
                            for (x, y, w) in dots.into_iter().flatten() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(at.x + px(x as f32), at.y + px(y as f32)),
                                        size(px(w as f32), px(w as f32)),
                                    ),
                                    color,
                                ));
                            }
                        }
                        // sem foco o Ghostty desenha o cursor vazado
                        if let Some((row, col)) = frame.hollow_cursor {
                            let at = point(
                                origin.x + cell.width * col as f32,
                                origin.y + cell.height * row as f32,
                            );
                            window.paint_quad(outline(
                                Bounds::new(at, cell),
                                cursor_color,
                                BorderStyle::Solid,
                            ));
                        }
                        for (row, segments) in frame.rows.into_iter().enumerate() {
                            for seg in segments {
                                let at = point(
                                    origin.x + cell.width * seg.col as f32,
                                    origin.y + cell.height * row as f32,
                                );
                                let ascii = seg.text.is_ascii();
                                let shaped = window.text_system().shape_line(
                                    seg.text.into(),
                                    font_size,
                                    &seg.runs,
                                    None,
                                );
                                if !ascii {
                                    let _ = shaped.paint(
                                        at,
                                        cell.height,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                    continue;
                                }
                                paint_on_grid(&shaped, &seg.runs, at, cell, font_size, window);
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

// texto digitado, inclusive o resultado de tecla morta (´ + e = é); o terminal não tem texto "marcado"
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input(text.as_bytes().to_vec());
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
