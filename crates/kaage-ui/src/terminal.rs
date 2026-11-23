use crate::theme::BEOS_TAB_YELLOW;
use egui::{Color32, CornerRadius, FontId, Key, Pos2, Rect, Vec2};
use kaage_proto::ClientMsg;

/// Pure-Rust VT100/xterm-256color/truecolor terminal pane.
///
/// Works identically in WebAssembly (with zero C/Zig toolchain needed inside
/// offline Nix WASM derivations) and on native desktop targets.
pub struct TerminalPane {
    pub session_id: u32,
    parser: vt100::Parser,
    pub cols: u16,
    pub rows: u16,
    pub cell_size: Vec2,
}

impl TerminalPane {
    pub fn new(session_id: u32, cols: u16, rows: u16) -> Self {
        let parser = vt100::Parser::new(rows, cols, 10_000);
        Self {
            session_id,
            parser,
            cols,
            rows,
            cell_size: Vec2::new(8.4, 17.0),
        }
    }

    pub fn feed_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, send_msg: &mut dyn FnMut(ClientMsg)) {
        let avail = ui.available_size();
        let new_cols = ((avail.x / self.cell_size.x).floor() as u16).max(20);
        let new_rows = ((avail.y / self.cell_size.y).floor() as u16).max(5);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;
            self.parser.screen_mut().set_size(new_rows, new_cols);
            send_msg(ClientMsg::ResizePty {
                session_id: self.session_id,
                cols: new_cols,
                rows: new_rows,
            });
        }

        let (rect, response) = ui.allocate_exact_size(avail, egui::Sense::click());
        if response.clicked() {
            ui.memory_mut(|m| m.request_focus(response.id));
        }

        if response.has_focus() {
            // Lock Escape, Tab, and Arrow keys to the terminal so egui never steals focus in Vim/tmux.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            let mut outbound = Vec::new();
            ui.input(|i| {
                for event in &i.events {
                    match event {
                        egui::Event::Text(text) => outbound.extend_from_slice(text.as_bytes()),
                        egui::Event::Paste(text) => outbound.extend_from_slice(text.as_bytes()),
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if modifiers.ctrl {
                                if let Some(b) = ctrl_key_to_byte(*key) {
                                    outbound.push(b);
                                }
                            } else {
                                match key {
                                    Key::Enter => outbound.push(b'\r'),
                                    Key::Backspace => outbound.push(0x7f),
                                    Key::Tab => {
                                        if modifiers.shift {
                                            outbound.extend_from_slice(b"\x1b[Z");
                                        } else {
                                            outbound.push(b'\t');
                                        }
                                    }
                                    Key::Escape => outbound.push(0x1b),
                                    Key::ArrowUp => outbound.extend_from_slice(if modifiers.alt {
                                        b"\x1b[1;3A".as_slice()
                                    } else {
                                        b"\x1b[A".as_slice()
                                    }),
                                    Key::ArrowDown => outbound.extend_from_slice(if modifiers.alt {
                                        b"\x1b[1;3B".as_slice()
                                    } else {
                                        b"\x1b[B".as_slice()
                                    }),
                                    Key::ArrowRight => outbound.extend_from_slice(if modifiers.alt {
                                        b"\x1b[1;3C".as_slice()
                                    } else {
                                        b"\x1b[C".as_slice()
                                    }),
                                    Key::ArrowLeft => outbound.extend_from_slice(if modifiers.alt {
                                        b"\x1b[1;3D".as_slice()
                                    } else {
                                        b"\x1b[D".as_slice()
                                    }),
                                    Key::Home => outbound.extend_from_slice(b"\x1b[H"),
                                    Key::End => outbound.extend_from_slice(b"\x1b[F"),
                                    Key::Delete => outbound.extend_from_slice(b"\x1b[3~"),
                                    Key::PageUp => outbound.extend_from_slice(b"\x1b[5~"),
                                    Key::PageDown => outbound.extend_from_slice(b"\x1b[6~"),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });

            if !outbound.is_empty() {
                send_msg(ClientMsg::PtyInput {
                    session_id: self.session_id,
                    data: outbound,
                });
            }
        }

        let default_bg = Color32::from_rgb(18, 20, 24);
        let default_fg = Color32::from_rgb(232, 232, 232);

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, default_bg);

        let font = FontId::monospace(14.0);
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();

        for r in 0..rows {
            let y = rect.min.y + (r as f32 * self.cell_size.y);
            for c in 0..cols {
                let Some(cell) = screen.cell(r, c) else {
                    continue;
                };
                let x = rect.min.x + (c as f32 * self.cell_size.x);
                let cell_rect = Rect::from_min_size(Pos2::new(x, y), self.cell_size);

                let (mut fg, mut bg) = (
                    vt_color_to_egui(cell.fgcolor(), default_fg),
                    vt_color_to_egui(cell.bgcolor(), default_bg),
                );
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }

                if bg != default_bg {
                    painter.rect_filled(cell_rect, CornerRadius::ZERO, bg);
                }

                let contents = cell.contents();
                if !contents.is_empty() && contents != " " {
                    painter.text(
                        cell_rect.min,
                        egui::Align2::LEFT_TOP,
                        contents,
                        font.clone(),
                        fg,
                    );
                }
            }
        }

        if !screen.hide_cursor() {
            let (cur_row, cur_col) = screen.cursor_position();
            let cx = rect.min.x + (cur_col as f32 * self.cell_size.x);
            let cy = rect.min.y + (cur_row as f32 * self.cell_size.y);
            let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), self.cell_size);
            let alpha = if response.has_focus() { 185 } else { 80 };
            painter.rect_filled(
                cursor_rect,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(
                    BEOS_TAB_YELLOW.r(),
                    BEOS_TAB_YELLOW.g(),
                    BEOS_TAB_YELLOW.b(),
                    alpha,
                ),
            );
        }
    }
}

fn vt_color_to_egui(color: vt100::Color, default: Color32) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(idx) => xterm_256_color(idx),
    }
}

fn xterm_256_color(idx: u8) -> Color32 {
    const ANSI_16: [(u8, u8, u8); 16] = [
        (24, 26, 31),
        (224, 108, 117),
        (152, 195, 121),
        (229, 192, 123),
        (97, 175, 239),
        (198, 120, 221),
        (86, 182, 194),
        (215, 218, 224),
        (92, 99, 112),
        (244, 135, 143),
        (175, 218, 144),
        (245, 211, 148),
        (128, 196, 250),
        (215, 152, 236),
        (115, 207, 218),
        (255, 255, 255),
    ];

    if idx < 16 {
        let (r, g, b) = ANSI_16[idx as usize];
        return Color32::from_rgb(r, g, b);
    }
    if idx < 232 {
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Color32::from_rgb(scale(r), scale(g), scale(b));
    }
    let gray = 8 + (idx - 232) * 10;
    Color32::from_rgb(gray, gray, gray)
}

fn ctrl_key_to_byte(key: Key) -> Option<u8> {
    match key {
        Key::A => Some(1),
        Key::B => Some(2),
        Key::C => Some(3),
        Key::D => Some(4),
        Key::E => Some(5),
        Key::F => Some(6),
        Key::G => Some(7),
        Key::H => Some(8),
        Key::K => Some(11),
        Key::L => Some(12),
        Key::N => Some(14),
        Key::P => Some(16),
        Key::R => Some(18),
        Key::T => Some(20),
        Key::U => Some(21),
        Key::V => Some(22),
        Key::W => Some(23),
        Key::X => Some(24),
        Key::Y => Some(25),
        Key::Z => Some(26),
        Key::OpenBracket => Some(27),  // Ctrl+[ == Escape (Vim-safe in Safari/Edge fullscreen)
        Key::Backslash => Some(28),    // Ctrl+\ (SIGQUIT)
        Key::CloseBracket => Some(29), // Ctrl+]
        _ => None,
    }
}
