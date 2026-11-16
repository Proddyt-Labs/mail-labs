// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Tasks board in the window: its columns, a card dragged to another
//! column, completed and reopened, and the choice of board kept with the window's settings.

use egui::{Event, Modifiers, PointerButton, Pos2};
use mailcraft_engine::TaskFilter;
use mailcraft_engine::cmd::tasks::BoardColumn;
use mailcraft_model::Id;
use serde_json::json;

use crate::people_view::testing::{self, Window};
use crate::tasks_board::{DONE_MAX, columns, heading};

fn board() -> Window {
    let mut w = testing::window("view.tasks");
    w.state_mut().execute("view.taskLayout", json!({"value": "board"})).unwrap();
    w.run_steps(3);
    w
}

fn new_task(w: &mut Window, title: &str, categories: &[&str]) -> Id {
    let id = w.state_mut().execute("tasks.new", json!({"title": title, "categories": categories, "account": 0})).unwrap()["id"].as_u64().unwrap();
    // (Nothing selected: the details pane stays shut and the four columns fit the window.)
    w.state_mut().execute("tasks.select", json!({"id": null})).unwrap();
    w.run_steps(3);
    id
}

fn column_of(w: &Window, id: Id) -> Option<BoardColumn> {
    let cols = columns(w.state());
    BoardColumn::ALL.into_iter().zip(cols.ids.iter()).find(|(_, ids)| ids.contains(&id)).map(|(c, _)| c)
}

/// Press on `from`, move a step at a time to `to`, let go there.
fn drag(w: &mut Window, from: Pos2, to: Pos2) {
    w.run_steps(30);
    w.input_mut().events.push(Event::PointerMoved(from));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    for i in 1..=10 {
        let at = from + (to - from) * (i as f32 / 10.0);
        w.input_mut().events.push(Event::PointerMoved(at));
        w.step();
    }
    w.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.run_steps(3);
}

/// Drag the card of `id` to the column `to`.
fn drag_to(w: &mut Window, id: Id, to: BoardColumn) {
    let card = w.state().hit.rect("taskCard", id).unwrap_or_else(|| panic!("no card for task {id}")).center();
    let i = BoardColumn::ALL.iter().position(|c| *c == to).unwrap() as u64;
    let column = w.state().hit.rect("taskBoardColumn", i).unwrap();
    drag(w, card, egui::pos2(column.center().x, column.min.y + 60.0));
}

#[test]
fn every_task_shown_stands_in_one_column() {
    let mut w = board();
    let plain = new_task(&mut w, "Ler capítulo", &["GNT"]);
    let going = new_task(&mut w, "Projeto AP1", &["AP1", "Em andamento"]);
    let closed = new_task(&mut w, "Reunião", &["fechada"]);
    assert_eq!(column_of(&w, plain), Some(BoardColumn::ToDo));
    assert_eq!(column_of(&w, going), Some(BoardColumn::InProgress));
    assert_eq!(column_of(&w, closed), Some(BoardColumn::Closed));
    // Every open task the list shows is on the board once, and a card is drawn for it.
    let cols = columns(w.state());
    let open: Vec<Id> = mailcraft_engine::cmd::tasks::sorted(&w.state().session, TaskFilter::Active);
    let mut on_board: Vec<Id> = cols.ids.iter().take(3).flatten().copied().collect();
    on_board.sort_unstable();
    let mut listed = open.clone();
    listed.sort_unstable();
    assert_eq!(on_board, listed);
    assert!(w.state().hit.rect("taskCard", plain).is_some());
    assert_eq!(heading(BoardColumn::Done), "Concluída");
    assert_eq!(heading(BoardColumn::InProgress), "Em andamento");
}

#[test]
fn a_card_dragged_to_another_column_takes_its_status() {
    let mut w = board();
    let id = new_task(&mut w, "Prova de GNT", &["GNT"]);
    drag_to(&mut w, id, BoardColumn::InProgress);
    let task = w.state().session.mailbox.task(id).cloned().unwrap();
    assert_eq!(task.categories, ["Em andamento", "GNT"]);
    assert_eq!(column_of(&w, id), Some(BoardColumn::InProgress));
    drag_to(&mut w, id, BoardColumn::Closed);
    assert_eq!(w.state().session.mailbox.task(id).unwrap().categories, ["Fechada", "GNT"]);
}

#[test]
fn dragging_to_concluida_completes_and_out_of_it_reopens() {
    let mut w = board();
    let id = new_task(&mut w, "Entrega", &["AP1", "Em andamento"]);
    drag_to(&mut w, id, BoardColumn::Done);
    assert!(w.state().session.mailbox.task(id).unwrap().is_complete());
    // Under Active, the latest completed tasks still stand in Concluída.
    assert_eq!(column_of(&w, id), Some(BoardColumn::Done));
    drag_to(&mut w, id, BoardColumn::ToDo);
    let task = w.state().session.mailbox.task(id).cloned().unwrap();
    assert!(!task.is_complete());
    assert_eq!(task.categories, ["A fazer", "AP1"]);
}

#[test]
fn concluida_shows_the_latest_and_counts_the_rest() {
    let mut w = board();
    for i in 0..(DONE_MAX + 3) {
        let id = new_task(&mut w, &format!("feita {i}"), &[]);
        w.state_mut().execute("tasks.complete", json!({"id": id, "value": true})).unwrap();
    }
    w.run_steps(2);
    let cols = columns(w.state());
    assert_eq!(cols.ids[3].len(), DONE_MAX);
    assert!(cols.more_done >= 3);
}

#[test]
fn the_filters_and_the_account_still_hold() {
    let mut w = board();
    let late = new_task(&mut w, "Atrasada", &[]);
    w.state_mut().execute("tasks.edit", json!({"id": late, "due": "2026-10-01"})).unwrap();
    let later = new_task(&mut w, "Depois", &[]);
    w.state_mut().execute("tasks.edit", json!({"id": later, "due": "2026-12-01"})).unwrap();
    w.state_mut().execute("view.taskFilter", json!({"value": "overdue"})).unwrap();
    w.run_steps(2);
    assert_eq!(column_of(&w, late), Some(BoardColumn::ToDo));
    assert_eq!(column_of(&w, later), None);
}

#[test]
fn the_choice_of_board_is_kept_with_the_windows_settings() {
    let w = board();
    let saved = w.state().prefs();
    assert!(saved.view.task_board);
    let mut again = testing::window("view.tasks");
    again.state_mut().apply_prefs(saved);
    assert!(again.state().session.view.task_board);
}
