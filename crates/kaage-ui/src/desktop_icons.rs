use crate::theme::{
    BEOS_BORDER_BLACK, BEOS_LED_GREEN, BEOS_PANEL_GRAY, BEOS_TAB_YELLOW, TOPBAR_HEIGHT,
};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};

pub enum DesktopIconAction {
    OpenHostServices,
    OpenSystemMonitor,
    OpenAiJobs,
    OpenHttpPorts,
    OpenTailscale,
    OpenTerminal,
}

/// Classic BeOS desktop shortcut icons along the right edge of the desktop.
pub fn show_desktop_icons(ctx: &egui::Context) -> Option<DesktopIconAction> {
    let screen = ctx.content_rect();
    if screen.width() < 760.0 {
        return None;
    }

    let mut action = None;
    let col_w = 78.0;
    let start = Pos2::new(screen.max.x - col_w - 14.0, TOPBAR_HEIGHT + 16.0);

    egui::Area::new(Id::unique("kaage_desktop_icons"))
        .fixed_pos(start)
        .order(egui::Order::Background)
        .show(ctx, |ui| {
            let items = [
                ("Services", IconKind::Folder, DesktopIconAction::OpenHostServices),
                ("Monitor", IconKind::Disk, DesktopIconAction::OpenSystemMonitor),
                ("AI Jobs", IconKind::Container, DesktopIconAction::OpenAiJobs),
                ("HTTP Ports", IconKind::Log, DesktopIconAction::OpenHttpPorts),
                ("Tailscale", IconKind::Disk, DesktopIconAction::OpenTailscale),
                ("Terminal", IconKind::Terminal, DesktopIconAction::OpenTerminal),
            ];

            for (idx, (label, kind, act)) in items.into_iter().enumerate() {
                let item_rect = Rect::from_min_size(
                    Pos2::new(start.x, start.y + idx as f32 * 70.0),
                    Vec2::new(col_w, 60.0),
                );
                let resp = ui.interact(
                    item_rect,
                    Id::unique(("desktop_icon", idx)),
                    Sense::click(),
                );
                let painter = ui.painter();

                if resp.hovered() {
                    painter.rect_filled(
                        item_rect,
                        CornerRadius::ZERO,
                        Color32::from_rgba_unmultiplied(255, 203, 0, 48),
                    );
                    painter.rect_stroke(
                        item_rect,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 203, 0, 180)),
                        egui::StrokeKind::Inside,
                    );
                }

                let icon_center = Pos2::new(item_rect.center().x, item_rect.min.y + 21.0);
                draw_icon(painter, icon_center, kind);

                // Label pill for crisp legibility over the wallpaper
                let label_rect = Rect::from_center_size(
                    Pos2::new(item_rect.center().x, item_rect.max.y - 10.0),
                    Vec2::new(col_w - 4.0, 15.0),
                );
                painter.rect_filled(
                    label_rect,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(20, 38, 62, 185),
                );
                painter.text(
                    label_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );

                if resp.clicked() {
                    action = Some(act);
                }
            }
        });

    action
}

#[derive(Clone, Copy)]
enum IconKind {
    Disk,
    Folder,
    Terminal,
    Container,
    Log,
}

fn draw_icon(painter: &egui::Painter, c: Pos2, kind: IconKind) {
    let stroke = Stroke::new(1.2, BEOS_BORDER_BLACK);
    match kind {
        IconKind::Disk => {
            // Isometric BeOS Drive
            let top = vec![
                Pos2::new(c.x - 13.0, c.y - 2.0),
                Pos2::new(c.x, c.y - 10.0),
                Pos2::new(c.x + 13.0, c.y - 2.0),
                Pos2::new(c.x, c.y + 6.0),
            ];
            let left = vec![
                Pos2::new(c.x - 13.0, c.y - 2.0),
                Pos2::new(c.x, c.y + 6.0),
                Pos2::new(c.x, c.y + 14.0),
                Pos2::new(c.x - 13.0, c.y + 6.0),
            ];
            let right = vec![
                Pos2::new(c.x + 13.0, c.y - 2.0),
                Pos2::new(c.x, c.y + 6.0),
                Pos2::new(c.x, c.y + 14.0),
                Pos2::new(c.x + 13.0, c.y + 6.0),
            ];
            painter.add(egui::Shape::convex_polygon(top, Color32::from_rgb(232, 232, 232), stroke));
            painter.add(egui::Shape::convex_polygon(left, Color32::from_rgb(184, 196, 208), stroke));
            painter.add(egui::Shape::convex_polygon(right, Color32::from_rgb(136, 152, 168), stroke));
            painter.circle_filled(Pos2::new(c.x + 7.0, c.y + 6.0), 2.2, BEOS_LED_GREEN);
        }
        IconKind::Folder => {
            // Classic yellow BeOS folder
            let tab = Rect::from_min_size(Pos2::new(c.x - 14.0, c.y - 11.0), Vec2::new(12.0, 5.0));
            let body = Rect::from_min_size(Pos2::new(c.x - 14.0, c.y - 7.0), Vec2::new(28.0, 19.0));
            painter.rect_filled(tab, CornerRadius::ZERO, BEOS_TAB_YELLOW);
            painter.rect_stroke(tab, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(body, CornerRadius::ZERO, Color32::from_rgb(255, 224, 96));
            painter.rect_stroke(body, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(body.min.x + 2.0, body.min.y + 2.0),
                    Pos2::new(body.max.x - 2.0, body.min.y + 2.0),
                ],
                Stroke::new(1.2, Color32::from_rgb(255, 246, 184)),
            );
        }
        IconKind::Terminal => {
            let outer = Rect::from_center_size(c, Vec2::new(28.0, 22.0));
            let screen = outer.shrink(3.0);
            painter.rect_filled(outer, CornerRadius::ZERO, BEOS_PANEL_GRAY);
            painter.rect_stroke(outer, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_rgb(24, 28, 34));
            painter.text(
                Pos2::new(screen.min.x + 3.0, screen.center().y),
                egui::Align2::LEFT_CENTER,
                ">_",
                egui::FontId::monospace(10.5),
                BEOS_TAB_YELLOW,
            );
        }
        IconKind::Container => {
            let box_r = Rect::from_center_size(c, Vec2::new(26.0, 20.0));
            painter.rect_filled(box_r, CornerRadius::ZERO, Color32::from_rgb(91, 136, 184));
            painter.rect_stroke(box_r, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            for dx in [-6.0, 0.0, 6.0] {
                painter.line_segment(
                    [Pos2::new(c.x + dx, box_r.min.y + 3.0), Pos2::new(c.x + dx, box_r.max.y - 3.0)],
                    Stroke::new(1.2, Color32::from_rgb(216, 232, 248)),
                );
            }
        }
        IconKind::Log => {
            let doc = Rect::from_center_size(c, Vec2::new(22.0, 26.0));
            let header = Rect::from_min_size(doc.min, Vec2::new(22.0, 5.0));
            painter.rect_filled(doc, CornerRadius::ZERO, Color32::from_rgb(252, 250, 240));
            painter.rect_filled(header, CornerRadius::ZERO, BEOS_TAB_YELLOW);
            painter.rect_stroke(doc, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            for dy in [-3.0, 2.0, 7.0] {
                painter.line_segment(
                    [Pos2::new(doc.min.x + 4.0, c.y + dy), Pos2::new(doc.max.x - 4.0, c.y + dy)],
                    Stroke::new(1.2, Color32::from_rgb(70, 80, 95)),
                );
            }
        }
    }
}
