//! Ruled tables and wrapped cells (F-PDF-18): grids from drawn lines, and rows whose cells wrap.

use crate::ruled::*;
use crate::table::{find_tables, table_markdown};
use crate::text::{Cell, TextLine};

fn line(y: f64, parts: &[(f64, &str)]) -> TextLine {
    let mut cells: Vec<Cell> = Vec::new();
    let mut end = 0.0;
    for (x, t) in parts {
        if !cells.is_empty() {
            cells.push(Cell { text: " ".into(), x: end, w: x - end });
        }
        let w = 6.0 * t.chars().count() as f64;
        cells.push(Cell { text: (*t).into(), x: *x, w });
        end = x + w;
    }
    TextLine { cells, y, size: 10.0 }
}

fn h(at: f64, a: f64, b: f64) -> Rule {
    Rule { at, a, b, horizontal: true }
}
fn v(at: f64, a: f64, b: f64) -> Rule {
    Rule { at, a, b, horizontal: false }
}

/// A grid of 3 columns (x 50, 150, 250, 350) and 3 rows (y 700, 670, 640, 610).
fn grid_rules() -> Vec<Rule> {
    let mut r = Vec::new();
    for y in [700.0, 670.0, 640.0, 610.0] {
        r.push(h(y, 50.0, 350.0));
    }
    for x in [50.0, 150.0, 250.0, 350.0] {
        r.push(v(x, 610.0, 700.0));
    }
    r
}

#[test]
fn drawn_lines_that_cross_make_one_grid_and_loose_lines_make_none() {
    let g = find_grids(&grid_rules());
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].xs, vec![50.0, 150.0, 250.0, 350.0]);
    assert_eq!(g[0].ys, vec![700.0, 670.0, 640.0, 610.0]);
    // an underline and a side bar that do not touch are not a grid
    assert!(find_grids(&[h(500.0, 50.0, 300.0), v(20.0, 400.0, 500.0), h(300.0, 50.0, 300.0)]).is_empty());
    // two rules only
    assert!(find_grids(&[h(500.0, 50.0, 300.0), v(60.0, 400.0, 500.0)]).is_empty());
}

#[test]
fn a_ruled_table_puts_each_text_in_its_cell_and_joins_the_lines_of_a_wrapped_cell() {
    let lines = vec![
        line(685.0, &[(55.0, "Name"), (155.0, "Role"), (255.0, "Years")]),
        line(655.0, &[(55.0, "Ada"), (155.0, "Engineer of"), (255.0, "12")]),
        line(645.0, &[(155.0, "machines")]),
        line(625.0, &[(55.0, "Bo"), (255.0, "3")]),
    ];
    let md = page_text_with_rules(&lines, &grid_rules());
    assert_eq!(md, "| Name | Role | Years |\n| --- | --- | --- |\n| Ada | Engineer of machines | 12 |\n| Bo |  | 3 |", "{md}");
}

#[test]
fn text_outside_the_grid_is_kept_before_and_after_and_a_drawing_with_no_text_is_not_a_table() {
    let mut lines = vec![line(740.0, &[(50.0, "Heading")])];
    lines.push(line(685.0, &[(55.0, "a"), (155.0, "b"), (255.0, "c")]));
    lines.push(line(655.0, &[(55.0, "d"), (155.0, "e"), (255.0, "f")]));
    lines.push(line(580.0, &[(50.0, "After")]));
    let md = page_text_with_rules(&lines, &grid_rules());
    assert_eq!(md, "Heading\n\n| a | b | c |\n| --- | --- | --- |\n| d | e | f |\n|  |  |  |\n\nAfter", "{md}");
    let none = vec![line(740.0, &[(50.0, "Only prose")])];
    assert_eq!(page_text_with_rules(&none, &grid_rules()), "Only prose");
}

#[test]
fn hostile_rules_do_not_panic() {
    let mut r = grid_rules();
    r.push(h(f64::NAN, 0.0, 1.0));
    r.push(v(f64::INFINITY, 0.0, 1.0));
    let _ = find_grids(&r);
    let many: Vec<Rule> = (0..MAX_RULES + 1).map(|i| h(i as f64 * 10.0, 0.0, 50.0)).collect();
    assert!(find_grids(&many).is_empty());
    assert_eq!(page_text_with_rules(&[], &[]), "");
}

#[test]
fn a_real_page_with_strokes_and_thin_fills_gives_a_ruled_markdown_table() {
    use crate::tests_support::simple;
    // three columns at x 10, 70, 130, 190 and two rows between y 90, 60, 30; borders: strokes for the verticals, thin fills for the horizontals
    let content = "0.5 w 10 90 m 10 30 l S 70 90 m 70 30 l S 130 90 m 130 30 l S 190 90 m 190 30 l S \
                   10 89.5 180 1 re f 10 59.5 180 1 re f 10 29.5 180 1 re f \
                   BT /F1 10 Tf 14 70 Td (Item) Tj ET BT /F1 10 Tf 74 70 Td (Qty) Tj ET BT /F1 10 Tf 134 70 Td (Cost) Tj ET \
                   BT /F1 10 Tf 14 40 Td (Tea) Tj ET BT /F1 10 Tf 74 46 Td (two) Tj ET BT /F1 10 Tf 74 36 Td (cups) Tj ET BT /F1 10 Tf 134 40 Td (3.50) Tj ET";
    let d = crate::Document::open(simple(&[content])).unwrap();
    assert_eq!(d.page_rules(0).unwrap().len(), 7);
    let t = d.page_text_tables(0).unwrap();
    assert_eq!(t, "| Item | Qty | Cost |\n| --- | --- | --- |\n| Tea | two cups | 3.50 |", "{t}");
}

#[test]
fn a_wrapped_cell_without_rules_joins_the_row_above_and_a_tight_trailing_line_does_not_count_as_a_row() {
    let lines = vec![
        line(700.0, &[(50.0, "Item"), (200.0, "Note"), (330.0, "Qty")]),
        line(684.0, &[(50.0, "Apple"), (200.0, "Red and"), (330.0, "3")]),
        line(674.0, &[(200.0, "sweet")]),
        line(658.0, &[(50.0, "Pear"), (200.0, "Green"), (330.0, "12")]),
    ];
    let t = find_tables(&lines);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].rows.len(), 3);
    assert_eq!(t[0].rows[1], vec!["Apple", "Red and sweet", "3"]);
    assert_eq!(t[0].rows[2], vec!["Pear", "Green", "12"]);
    assert!(table_markdown(&t[0]).contains("| Apple | Red and sweet | 3 |"));
}
