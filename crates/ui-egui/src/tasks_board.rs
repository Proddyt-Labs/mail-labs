// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Tasks as a board, beside its list (`view.taskLayout`): a column for each
//! status the owner keeps as a category (A fazer, Em andamento, Fechada, the names Google Tasks
//! writes as `? `, `▶ ` and `✗ `) and one for the completed tasks (Concluída).
//!
//! The board shows what the list would: the filter of the side pane and the account chosen hold
//! (under Active, the default, the most recently completed tasks are shown in Concluída all the
//! same, so that there is somewhere to drop a card). An open task with no status category stands
//! in A fazer. Dragging a card to another column is `tasks.setStatus`, the same edit an agent
//! makes: the status category is replaced, Concluída marks the task complete and leaving it
//! reopens it; the account's server is told as for any other change.

use egui::{Align2, Color32, LayerId, Order, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use mailcraft_engine::TaskFilter;
use mailcraft_engine::cmd::tasks::{self, BoardColumn};
use mailcraft_model::{Id, Task};
use mailcraft_time::Date;
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

/// The most completed tasks Concluída shows; the rest are counted below it.
pub(crate) const DONE_MAX: usize = 25;
/// A card's height, the gap between columns, and a column's narrowest.
const CARD_H: f32 = 62.0;
const GAP: f32 = 8.0;
const COLUMN_MIN: f32 = 190.0;
/// The height of a column's heading.
const HEAD_H: f32 = 30.0;
/// Where the window keeps the task whose card is being dragged.
const DRAG_SLOT: &str = "task_board_drag";

/// What the board shows: the tasks of each column, in [`BoardColumn::ALL`] order, and how many
/// completed tasks Concluída leaves out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Columns {
    pub(crate) ids: [Vec<Id>; 4],
    pub(crate) more_done: usize,
}

/// The board's columns for the filter and the account in view: open tasks in the list's order,
/// completed ones most recently completed first.
pub(crate) fn columns(app: &MailApp) -> Columns {
    let s = &app.session;
    let filter = s.view.task_filter;
    let mut out = Columns::default();
    let mut done: Vec<Id> = Vec::new();
    for id in tasks::sorted(s, filter) {
        let Some(task) = s.mailbox.task(id) else { continue };
        match BoardColumn::of(task) {
            BoardColumn::Done => done.push(id),
            column => {
                if let Some(ids) = out.ids.get_mut(index(column)) {
                    ids.push(id);
                }
            }
        }
    }
    // Under Active the completed ones aren't in the list; the board shows the latest of them.
    if filter == TaskFilter::Active {
        done = tasks::sorted(s, TaskFilter::Completed);
    }
    out.more_done = done.len().saturating_sub(DONE_MAX);
    done.truncate(DONE_MAX);
    if let Some(ids) = out.ids.get_mut(index(BoardColumn::Done)) {
        *ids = done;
    }
    out
}

fn index(column: BoardColumn) -> usize {
    BoardColumn::ALL.iter().position(|c| *c == column).unwrap_or(0)
}

/// What a column's heading says: the status category's own name, or Concluída (the owner's
/// words, as the categories are, not the interface's).
pub(crate) fn heading(column: BoardColumn) -> &'static str {
    column.category().unwrap_or("Concluída")
}

/// The categories a card shows as chips: those that are not a status.
fn tags(task: &Task) -> impl Iterator<Item = &String> {
    task.categories.iter().filter(|c| !mailcraft_model::is_status_category(c))
}

/// The board, in the place of the list.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let filter = app.session.view.task_filter;
    let cols = columns(app);
    let open: usize = cols.ids.iter().take(3).map(Vec::len).sum();
    crate::chrome::pane_title(ui, tl!(filter.label()), &open.to_string());
    crate::tasks_view::quick_add(app, ui);
    let top = ui.available_rect_before_wrap();
    ui.painter().line_segment([pos2(top.min.x, top.min.y), pos2(top.max.x, top.min.y)], Stroke::new(1.0, t.border));
    let today = app.session.today();
    let mut dropped: Option<(Id, BoardColumn)> = None;
    // Too narrow for four columns side by side: the board scrolls sideways.
    let width = ((top.width() - GAP * 5.0) / 4.0).max(COLUMN_MIN);
    let whole = vec2((width * 4.0 + GAP * 5.0).max(top.width()), top.height());
    egui::ScrollArea::horizontal().id_salt("task_board_across").auto_shrink([false, false]).show(ui, |ui| {
        let (area, _) = ui.allocate_exact_size(whole, Sense::hover());
        let clip = ui.clip_rect();
        let rects: Vec<Rect> = (0..4u8)
            .map(|i| {
                let x = area.min.x + GAP + (width + GAP) * f32::from(i);
                Rect::from_min_max(pos2(x, area.min.y + GAP), pos2(x + width, (area.max.y - GAP).max(area.min.y + GAP + HEAD_H)))
            })
            .collect();
        // Where a card may be dropped: the part of each column in view.
        let targets: Vec<Rect> = rects.iter().map(|r| r.intersect(clip)).collect();
        let dragged = ui.ctx().data(|d| d.get_temp::<Id>(egui::Id::unique(DRAG_SLOT)));
        let pointer = ui.ctx().pointer_latest_pos();
        for (i, column) in BoardColumn::ALL.into_iter().enumerate() {
            let (Some(rect), Some(target_rect)) = (rects.get(i).copied(), targets.get(i).copied()) else { continue };
            app.hit.add("taskBoardColumn", i as u64, target_rect);
            let target = dragged.is_some() && pointer.is_some_and(|p| target_rect.contains(p));
            ui.painter().rect(
                rect,
                6.0,
                if target { t.accent_soft } else { t.surface_sunken },
                Stroke::new(1.0, if target { t.accent } else { t.border_faint }),
                egui::StrokeKind::Inside,
            );
            let ids = cols.ids.get(i).cloned().unwrap_or_default();
            let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
            let count = ids.len() + if column == BoardColumn::Done { cols.more_done } else { 0 };
            let number_w = ui
                .painter()
                .text(pos2(head.max.x - 10.0, head.center().y), Align2::RIGHT_CENTER, count.to_string(), semibold(11.5), t.text_dim)
                .width();
            paint_clipped(
                ui.painter(),
                pos2(head.min.x + 10.0, head.center().y),
                (head.width() - 30.0 - number_w).max(0.0),
                heading(column),
                semibold(12.5),
                t.text,
            );
            let body = Rect::from_min_max(pos2(rect.min.x + 6.0, head.max.y), pos2(rect.max.x - 6.0, (rect.max.y - 4.0).max(head.max.y)));
            let mut child = ui.new_child(UiBuilder::new().max_rect(body));
            egui::ScrollArea::vertical().id_salt(("task_board", i)).auto_shrink([false, false]).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
                for id in &ids {
                    if let Some(to) = card(app, ui, *id, today, &targets) {
                        dropped = Some((*id, to));
                    }
                }
                if column == BoardColumn::Done && cols.more_done > 0 {
                    let more = tln!(cols.more_done, "{n} more completed task", "{n} more completed tasks");
                    ui.label(egui::RichText::new(more).font(regular(11.5)).color(t.text_dim));
                }
                if ids.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tl!("Drop a task here")).font(regular(11.5)).color(t.text_disabled));
                }
            });
        }
    });
    if let Some((id, column)) = dropped {
        let now = app.session.mailbox.task(id).map(BoardColumn::of);
        if now != Some(column) {
            let _ = app.run("tasks.setStatus", json!({"id": id, "status": column.code()}));
        }
    }
}

/// One card. Returns the column it was dropped on, when a drag of it ended over one.
fn card(app: &mut MailApp, ui: &mut Ui, id: Id, today: Date, columns: &[Rect]) -> Option<BoardColumn> {
    let t = Tokens::get(ui.ctx());
    let task = app.session.mailbox.tasks.get(&id).cloned()?;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H), Sense::click_and_drag());
    app.hit.add("taskCard", id, rect);
    let slot = egui::Id::unique(DRAG_SLOT);
    if resp.drag_started() {
        ui.ctx().data_mut(|d| d.insert_temp(slot, id));
    }
    let selected = app.session.sel.task == Some(id);
    if resp.dragged() {
        // The card follows the pointer, above everything; its place is left faint.
        ui.painter().rect_filled(rect, 5.0, t.hover_subtle);
        if let Some(at) = ui.ctx().pointer_latest_pos() {
            let painter = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, egui::Id::unique("task_board_ghost")));
            paint_card(app, &painter, Rect::from_center_size(at, rect.size()), &task, today, true, &t);
        }
    } else {
        paint_card(app, ui.painter(), rect, &task, today, selected || resp.hovered(), &t);
    }
    if resp.clicked() {
        let _ = app.run("tasks.select", json!({"id": id}));
    }
    if resp.double_clicked() {
        let _ = app.execute("tasks.select", json!({"id": id}));
        let _ = app.run("tasks.edit", json!({}));
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Id>(slot));
        let at = ui.ctx().pointer_latest_pos()?;
        let i = columns.iter().position(|r| r.contains(at))?;
        return BoardColumn::ALL.get(i).copied();
    }
    None
}

/// What a card shows: the account's colour, the title, when it is due, and its other categories.
fn paint_card(app: &MailApp, p: &egui::Painter, rect: Rect, task: &Task, today: Date, lit: bool, t: &Tokens) {
    let done = task.is_complete();
    p.rect(rect, 5.0, t.surface, Stroke::new(1.0, if lit { t.accent } else { t.border }), egui::StrokeKind::Inside);
    let mut left = rect.min.x + 10.0;
    let top = rect.min.y + 17.0;
    if let Some(color) = app.session.mailbox.account(task.origin.account).and_then(|a| a.color) {
        p.circle_filled(pos2(left + 4.0, top), 4.0, crate::theme::c32(color));
        left += 14.0;
    }
    let title = if task.title.trim().is_empty() { tl!("(No title)").to_string() } else { crate::people_view::one_line(&task.title, 200) };
    let color = if done { t.text_dim } else { t.text };
    let w = paint_clipped(p, pos2(left, top), (rect.max.x - left - 10.0).max(0.0), &title, semibold(12.5), color);
    if done {
        p.line_segment([pos2(left, top), pos2(left + w, top)], Stroke::new(1.0, t.text_dim));
    }
    // The second line: when it is due, then the chips, as far as they fit.
    let y = rect.max.y - 18.0;
    let mut x = rect.min.x + 10.0;
    let due = crate::tasks_view::due_text(task, today);
    if !due.is_empty() {
        icons::paint(p, Rect::from_center_size(pos2(x + 6.0, y), vec2(12.0, 12.0)), "calendar", t.text_dim, t.accent);
        let late = task.is_overdue(today);
        x += 16.0 + p.text(pos2(x + 16.0, y), Align2::LEFT_CENTER, &due, regular(11.0), if late { t.red } else { t.text_dim }).width() + 8.0;
    }
    for name in tags(task).take(6) {
        let shown = crate::people_view::one_line(name, 24);
        let w = p.layout_no_wrap(shown.clone(), regular(10.5), Color32::WHITE).size().x + 12.0;
        if x + w > rect.max.x - 8.0 {
            break;
        }
        widgets::category_chip_on(p, pos2(x, y), &shown, crate::theme::c32(app.session.mailbox.category_color_of(task.origin.account, name)), t);
        x += w + 4.0;
    }
}
