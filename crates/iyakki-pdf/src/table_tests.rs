//! Table analysis on lines built by hand (positions in points, font size 10).

use crate::table::*;
use crate::text::{Cell, TextLine};

/// A line whose fields start at the given x values; each field is one cell of 6 points per letter, spaces between fields are cells.
fn row(y: f64, fields: &[(f64, &str)]) -> TextLine {
    let mut cells: Vec<Cell> = Vec::new();
    let mut end = 0.0;
    for (x, t) in fields {
        if !cells.is_empty() {
            cells.push(Cell { text: " ".into(), x: end, w: x - end });
        }
        let w = 6.0 * t.chars().count() as f64;
        cells.push(Cell { text: (*t).into(), x: *x, w });
        end = x + w;
    }
    TextLine { cells, y, size: 10.0 }
}

fn prose(y: f64, t: &str) -> TextLine {
    row(y, &[(50.0, t)])
}

#[test]
fn rows_that_share_columns_are_a_table_and_read_as_cells() {
    let lines = vec![
        row(700.0, &[(50.0, "Item"), (200.0, "Qty"), (300.0, "Price")]),
        row(686.0, &[(50.0, "Apple"), (200.0, "3"), (300.0, "1.20")]),
        row(672.0, &[(50.0, "Pear tree"), (200.0, "12"), (300.0, "15.00")]),
    ];
    let t = find_tables(&lines);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].rows[0], vec!["Item", "Qty", "Price"]);
    assert_eq!(t[0].rows[2], vec!["Pear tree", "12", "15.00"]);
    let md = table_markdown(&t[0]);
    assert!(md.starts_with("| Item | Qty | Price |\n| --- | --- | --- |\n| Apple | 3 | 1.20 |"), "{md}");
}

#[test]
fn right_aligned_numbers_stay_in_their_column_and_empty_cells_are_kept() {
    let lines = vec![
        row(700.0, &[(50.0, "Name"), (260.0, "Total")]),
        row(686.0, &[(50.0, "One"), (284.0, "5")]),
        row(672.0, &[(50.0, "Two"), (272.0, "120")]),
        row(658.0, &[(50.0, "Three")]),
    ];
    // the last line has one field only: the block ends before it
    let t = find_tables(&lines);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].rows.len(), 3);
    assert_eq!(t[0].rows[1], vec!["One", "5"]);
    assert_eq!(t[0].rows[2], vec!["Two", "120"]);
    assert_eq!((t[0].first_line, t[0].last_line), (0, 2));
}

#[test]
fn prose_and_two_lines_are_not_tables() {
    let lines = vec![prose(700.0, "A paragraph of plain words"), prose(686.0, "that goes on"), prose(672.0, "for three lines")];
    assert!(find_tables(&lines).is_empty());
    let two = vec![row(700.0, &[(50.0, "a"), (200.0, "b")]), row(686.0, &[(50.0, "c"), (200.0, "d")])];
    assert!(find_tables(&two).is_empty());
}

#[test]
fn a_field_across_the_gutter_makes_one_column_and_no_table() {
    let lines = vec![
        row(700.0, &[(50.0, "A heading that runs across the whole page width of the text block")]),
        row(686.0, &[(50.0, "left words here"), (300.0, "right words")]),
        row(672.0, &[(50.0, "more of a long left column that reaches far into the right"), (400.0, "x")]),
    ];
    // only the last two lines have two fields: too few rows
    assert!(find_tables(&lines).is_empty());
}

#[test]
fn text_around_a_table_is_kept_and_the_table_is_written_in_place() {
    let lines = vec![
        prose(720.0, "Before the table"),
        row(700.0, &[(50.0, "K"), (200.0, "V")]),
        row(686.0, &[(50.0, "a"), (200.0, "1")]),
        row(672.0, &[(50.0, "b"), (200.0, "2")]),
        prose(640.0, "After the table"),
    ];
    let t = page_text_with_tables(&lines);
    assert_eq!(t, "Before the table\n\n| K | V |\n| --- | --- |\n| a | 1 |\n| b | 2 |\n\nAfter the table");
}

#[test]
fn bars_and_hostile_cells_are_escaped_and_hostile_numbers_do_not_panic() {
    let lines = vec![
        row(700.0, &[(50.0, "a|b"), (200.0, "c")]),
        row(686.0, &[(50.0, "d"), (200.0, "e")]),
        row(672.0, &[(50.0, "f"), (200.0, "g")]),
    ];
    assert!(table_markdown(&find_tables(&lines)[0]).contains("a\\|b"));
    let mut bad = lines.clone();
    bad[1].y = f64::NAN;
    assert!(find_tables(&bad).is_empty());
    let mut zero = lines.clone();
    zero[0].size = 0.0;
    assert!(find_tables(&zero).is_empty());
    assert_eq!(page_text_with_tables(&[]), "");
}

#[test]
fn a_real_page_with_a_grid_of_words_gives_a_markdown_table() {
    use crate::tests_support::simple;
    let content = "BT /F1 10 Tf 20 80 Td (Item) Tj ET BT /F1 10 Tf 90 80 Td (Qty) Tj ET BT /F1 10 Tf 150 80 Td (Cost) Tj ET \
                   BT /F1 10 Tf 20 66 Td (Tea) Tj ET BT /F1 10 Tf 90 66 Td (2) Tj ET BT /F1 10 Tf 150 66 Td (3.50) Tj ET \
                   BT /F1 10 Tf 20 52 Td (Milk) Tj ET BT /F1 10 Tf 90 52 Td (11) Tj ET BT /F1 10 Tf 150 52 Td (9.00) Tj ET";
    let d = crate::Document::open(simple(&[content])).unwrap();
    let t = d.page_text_tables(0).unwrap();
    assert_eq!(t, "| Item | Qty | Cost |\n| --- | --- | --- |\n| Tea | 2 | 3.50 |\n| Milk | 11 | 9.00 |", "{t}");
    assert!(!d.page_text(0).unwrap().contains('|'));
}

#[test]
fn brackets_in_right_to_left_text_come_back_as_typed() {
    use crate::text::visual_to_logical;
    // the page shows ( then two Hebrew letters then ): the glyphs are mirrored, the typed text has the pair the same way round
    assert_eq!(visual_to_logical("(\u{5d1}\u{5d0})"), "(\u{5d0}\u{5d1})");
    // Latin and digits inside keep their order, and a line with no right-to-left letter is not touched, brackets included
    assert_eq!(visual_to_logical("\u{5d1}\u{5d0} 12 [ab]"), "[ab] 12 \u{5d0}\u{5d1}");
    assert_eq!(visual_to_logical("f(x) > [1]"), "f(x) > [1]");
}
