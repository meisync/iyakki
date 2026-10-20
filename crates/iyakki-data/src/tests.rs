use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use crate::model::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-data-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &std::path::Path, name: &str, text: &str) -> String {
    let p = d.join(name);
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

/// Every engine this build has: the same tests run against each, so they must agree.
fn engines() -> Vec<Arc<dyn DataService>> {
    #[allow(unused_mut)]
    let mut v: Vec<Arc<dyn DataService>> = vec![crate::builtin_service()];
    #[cfg(feature = "polars")]
    v.push(Arc::new(crate::polars_engine::PolarsEngine::new()));
    v
}

const SALES: &str = "id,region,product,qty,price,paid\n1,north,tea,3,2.5,true\n2,south,coffee,1,3.0,false\n3,north,coffee,,3.0,true\n4,east,tea,5,2.5,\n5,south,cake,2,4.75,true\n";

fn open(s: &dyn DataService, path: &str) -> DatasetInfo {
    s.open(&OpenRequest::new(path)).unwrap_or_else(|e| panic!("{} open: {e}", s.engine()))
}

fn page(s: &dyn DataService, p: Pipeline, offset: u64, limit: u64) -> Page {
    s.page(&PageRequest { pipeline: p, offset, limit }).unwrap_or_else(|e| panic!("{} page: {e}", s.engine()))
}

fn col(p: &Page, name: &str) -> Vec<Value> {
    let i = p.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name} in {:?}", p.columns));
    p.rows.iter().map(|r| r[i].clone()).collect()
}

#[test]
fn types_nulls_and_row_count() {
    let d = dir("types");
    let f = write(&d, "sales.csv", SALES);
    for s in engines() {
        let i = open(&*s, &f);
        let t: Vec<(&str, DType)> = i.columns.iter().map(|c| (c.name.as_str(), c.dtype)).collect();
        assert_eq!(t, vec![("id", DType::Int), ("region", DType::Str), ("product", DType::Str), ("qty", DType::Int), ("price", DType::Float), ("paid", DType::Bool)], "{}", s.engine());
        assert_eq!(i.rows, 5, "{}", s.engine());
        let p = page(&*s, Pipeline::new(&i.handle), 0, 10);
        assert_eq!(col(&p, "qty"), vec![json!(3), json!(1), Value::Null, json!(5), json!(2)], "{}", s.engine());
        assert_eq!(col(&p, "paid")[3], Value::Null, "{}", s.engine());
        assert_eq!(col(&p, "price")[0], json!(2.5));
        assert_eq!(p.total, 5);
    }
}

#[test]
fn quoted_newlines_bom_crlf_and_tabs() {
    let d = dir("quoted");
    let f = write(&d, "q.csv", "\u{feff}name,note\r\n\"Ann\",\"line one\nline two\"\r\n\"Bob, Jr.\",\"say \"\"hi\"\"\"\r\n");
    let t = write(&d, "t.tsv", "a\tb\n1\tx y\n2\t\"q\tr\"\n");
    for s in engines() {
        let i = open(&*s, &f);
        assert_eq!(i.columns[0].name, "name", "the byte order mark is not part of the name ({})", s.engine());
        assert_eq!(i.rows, 2, "{}", s.engine());
        let p = page(&*s, Pipeline::new(&i.handle), 0, 10);
        assert_eq!(col(&p, "note"), vec![json!("line one\nline two"), json!("say \"hi\"")], "{}", s.engine());
        assert_eq!(col(&p, "name")[1], json!("Bob, Jr."));
        let i = open(&*s, &t);
        let p = page(&*s, Pipeline::new(&i.handle), 0, 10);
        assert_eq!(col(&p, "b"), vec![json!("x y"), json!("q\tr")], "{}", s.engine());
        assert_eq!(col(&p, "a"), vec![json!(1), json!(2)]);
    }
}

#[test]
fn sort_filter_select_rename_derive_limit() {
    let d = dir("ops");
    let f = write(&d, "sales.csv", SALES);
    for s in engines() {
        let e = s.engine();
        let i = open(&*s, &f);
        let base = Pipeline::new(&i.handle);
        // filter: text, numbers, nulls
        let p = page(&*s, base.clone().with(Op::Filter { expr: "contains(product, \"COF\") and price >= 3".into() }), 0, 10);
        assert_eq!(col(&p, "id"), vec![json!(2), json!(3)], "{e}");
        assert_eq!(p.total, 2);
        let p = page(&*s, base.clone().with(Op::Filter { expr: "qty is null".into() }), 0, 10);
        assert_eq!(col(&p, "id"), vec![json!(3)], "{e}");
        let p = page(&*s, base.clone().with(Op::Filter { expr: "region in (\"north\", \"east\") and not paid".into() }), 0, 10);
        assert_eq!(p.total, 0, "{e}: a null never matches");
        // sort: descending, ties by the second key, nulls last either way
        let p = page(&*s, base.clone().with(Op::Sort { keys: vec![SortKey { column: "qty".into(), dir: Dir::Desc }] }), 0, 10);
        assert_eq!(col(&p, "id"), vec![json!(4), json!(1), json!(5), json!(2), json!(3)], "{e}");
        let p = page(&*s, base.clone().with(Op::Sort { keys: vec![SortKey { column: "price".into(), dir: Dir::Asc }, SortKey { column: "id".into(), dir: Dir::Desc }] }), 0, 10);
        assert_eq!(col(&p, "id"), vec![json!(4), json!(1), json!(3), json!(2), json!(5)], "{e}");
        let p = page(&*s, base.clone().with(Op::Sort { keys: vec![SortKey { column: "qty".into(), dir: Dir::Asc }] }), 0, 10);
        assert_eq!(col(&p, "id")[4], json!(3), "{e}: nulls last");
        // select + rename + derive + limit, in order
        let pipe = base
            .clone()
            .with(Op::Derive { name: "total".into(), expr: "qty * price".into() })
            .with(Op::Filter { expr: "total > 5".into() })
            .with(Op::Select { columns: vec!["id".into(), "total".into(), "region".into()] })
            .with(Op::Rename { map: vec![("region".into(), "area".into())] })
            .with(Op::Sort { keys: vec![SortKey { column: "total".into(), dir: Dir::Desc }] })
            .with(Op::Limit { n: 2 });
        let p = page(&*s, pipe.clone(), 0, 10);
        assert_eq!(p.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["id", "total", "area"], "{e}");
        assert_eq!(p.columns[1].dtype, DType::Float);
        assert_eq!(col(&p, "id"), vec![json!(4), json!(5)], "{e}");
        assert_eq!(col(&p, "total"), vec![json!(12.5), json!(9.5)], "{e}");
        assert_eq!(p.total, 2);
        let info = s.schema(&pipe).unwrap();
        assert_eq!(info.rows, 2, "{e}");
        // paging inside a result
        let p = page(&*s, base.clone().with(Op::Sort { keys: vec![SortKey { column: "id".into(), dir: Dir::Desc }] }), 3, 10);
        assert_eq!((col(&p, "id"), p.offset, p.total), (vec![json!(2), json!(1)], 3, 5), "{e}");
        let p = page(&*s, base.clone(), 99, 10);
        assert!(p.rows.is_empty() && p.total == 5, "{e}");
    }
}

#[test]
fn group_by_aggregates() {
    let d = dir("group");
    let f = write(&d, "sales.csv", SALES);
    for s in engines() {
        let e = s.engine();
        let i = open(&*s, &f);
        let g = Pipeline::new(&i.handle).with(Op::GroupBy {
            by: vec!["region".into()],
            aggs: vec![
                Agg { column: String::new(), func: AggFn::Count, alias: None },
                Agg { column: "qty".into(), func: AggFn::Sum, alias: Some("units".into()) },
                Agg { column: "price".into(), func: AggFn::Mean, alias: None },
                Agg { column: "product".into(), func: AggFn::Unique, alias: Some("kinds".into()) },
                Agg { column: "price".into(), func: AggFn::Max, alias: None },
            ],
        });
        let p = page(&*s, g.clone(), 0, 10);
        assert_eq!(col(&p, "region"), vec![json!("east"), json!("north"), json!("south")], "{e}: groups come in key order");
        assert_eq!(col(&p, "count"), vec![json!(1), json!(2), json!(2)], "{e}");
        assert_eq!(col(&p, "units"), vec![json!(5), json!(3), json!(3)], "{e}");
        assert_eq!(col(&p, "mean_price"), vec![json!(2.5), json!(2.75), json!(3.875)], "{e}");
        assert_eq!(col(&p, "kinds"), vec![json!(1), json!(2), json!(2)], "{e}");
        assert_eq!(col(&p, "max_price"), vec![json!(2.5), json!(3.0), json!(4.75)], "{e}");
        // steps after a group-by work on the groups
        let p = page(&*s, g.with(Op::Sort { keys: vec![SortKey { column: "units".into(), dir: Dir::Desc }] }).with(Op::Limit { n: 1 }), 0, 10);
        assert_eq!(col(&p, "region"), vec![json!("east")], "{e}");
        // no key: one row for the whole table
        let p = page(&*s, Pipeline::new(&i.handle).with(Op::GroupBy { by: vec![], aggs: vec![Agg { column: "price".into(), func: AggFn::Sum, alias: Some("revenue".into()) }] }), 0, 10);
        assert_eq!(col(&p, "revenue"), vec![json!(15.75)], "{e}");
        // a sum of text is refused
        let err = s.page(&PageRequest { pipeline: Pipeline::new(&i.handle).with(Op::GroupBy { by: vec![], aggs: vec![Agg { column: "region".into(), func: AggFn::Sum, alias: None }] }), offset: 0, limit: 5 }).unwrap_err();
        assert!(err.to_string().contains("number column"), "{e}: {err}");
    }
}

#[test]
fn join_two_files() {
    let d = dir("join");
    let a = write(&d, "a.csv", "id,name\n1,Ann\n2,Bob\n3,Cy\n");
    let b = write(&d, "b.csv", "id,city,name\n1,Paris,x\n3,Rome,y\n3,Oslo,z\n9,Nowhere,w\n");
    for s in engines() {
        let e = s.engine();
        let ia = open(&*s, &a);
        let ib = open(&*s, &b);
        let inner = Pipeline::new(&ia.handle).with(Op::Join { other: ib.handle.clone(), on: vec!["id".into()], kind: JoinKind::Inner });
        let p = page(&*s, inner.clone().with(Op::Sort { keys: vec![SortKey { column: "city".into(), dir: Dir::Asc }] }), 0, 10);
        assert_eq!(col(&p, "city"), vec![json!("Oslo"), json!("Paris"), json!("Rome")], "{e}");
        assert!(p.columns.iter().any(|c| c.name == "name_right"), "{e}: a clashing name gets a suffix: {:?}", p.columns);
        let left = Pipeline::new(&ia.handle).with(Op::Join { other: ib.handle.clone(), on: vec!["id".into()], kind: JoinKind::Left });
        let p = page(&*s, left, 0, 10);
        assert_eq!(p.total, 4, "{e}: Bob has no match but stays");
        // a result can be kept as a dataset and explored further
        let kept = s.materialize(&inner).unwrap();
        assert_eq!(kept.rows, 3, "{e}");
        let p = page(&*s, Pipeline::new(&kept.handle).with(Op::Filter { expr: "city = 'Rome'".into() }), 0, 10);
        assert_eq!(col(&p, "name"), vec![json!("Cy")], "{e}");
    }
}

#[test]
fn describe_gives_statistics() {
    let d = dir("describe");
    let f = write(&d, "sales.csv", SALES);
    for s in engines() {
        let e = s.engine();
        let i = open(&*s, &f);
        let sum = s.describe(&Pipeline::new(&i.handle)).unwrap();
        assert_eq!(sum.rows, 5);
        let qty = sum.columns.iter().find(|c| c.name == "qty").unwrap();
        assert_eq!((qty.count, qty.nulls, qty.unique), (4, 1, Some(4)), "{e}");
        assert_eq!((qty.min.clone(), qty.max.clone()), (Some(json!(1)), Some(json!(5))), "{e}");
        assert_eq!(qty.mean, Some(2.75), "{e}");
        assert_eq!(qty.median, Some(2.5), "{e}");
        let sd = qty.std.unwrap();
        assert!((sd - 1.7078).abs() < 1e-3, "{e}: {sd}");
        let region = sum.columns.iter().find(|c| c.name == "region").unwrap();
        assert_eq!(region.unique, Some(3), "{e}");
        assert_eq!(region.top[0].1, 2, "{e}");
        assert!(region.mean.is_none());
        // of a filtered result
        let sum = s.describe(&Pipeline::new(&i.handle).with(Op::Filter { expr: "region = 'north'".into() })).unwrap();
        assert_eq!(sum.rows, 2, "{e}");
        let md = crate::table::summary_markdown(&sum);
        assert!(md.contains("| qty | int |"), "{md}");
    }
}

#[test]
fn export_round_trips_through_a_file() {
    let d = dir("export");
    let f = write(&d, "q.csv", "name,note,n\nAnn,\"a,b\",1\nBob,\"line1\nline2\",2\nCy,\"q\"\"x\",3\n");
    for s in engines() {
        let e = s.engine();
        let i = open(&*s, &f);
        let out = d.join(format!("out-{e}.csv")).to_string_lossy().into_owned();
        let r = s.export_csv(&ExportRequest { pipeline: Pipeline::new(&i.handle).with(Op::Filter { expr: "n >= 2".into() }).with(Op::Sort { keys: vec![SortKey { column: "n".into(), dir: Dir::Desc }] }), path: out.clone() }).unwrap();
        assert_eq!(r.rows, 2, "{e}");
        let back = open(&*s, &out);
        let p = page(&*s, Pipeline::new(&back.handle), 0, 10);
        assert_eq!(col(&p, "name"), vec![json!("Cy"), json!("Bob")], "{e}");
        assert_eq!(col(&p, "note"), vec![json!("q\"x"), json!("line1\nline2")], "{e}");
        assert!(r.bytes > 10);
    }
}

#[test]
fn mistakes_come_back_as_plain_sentences() {
    let d = dir("errors");
    let f = write(&d, "sales.csv", SALES);
    for s in engines() {
        let e = s.engine();
        let missing = s.open(&OpenRequest::new(d.join("nope.csv").to_string_lossy())).unwrap_err().to_string();
        assert!(missing.contains("no such file"), "{e}: {missing}");
        let empty = write(&d, "empty.csv", "");
        assert!(s.open(&OpenRequest::new(empty)).unwrap_err().to_string().contains("empty"), "{e}");
        let i = open(&*s, &f);
        let try_op = |op: Op| s.page(&PageRequest { pipeline: Pipeline::new(&i.handle).with(op), offset: 0, limit: 5 }).unwrap_err().to_string();
        let m = try_op(Op::Filter { expr: "prce > 1".into() });
        assert!(m.contains("no column \"prce\"") && m.contains("price"), "{e}: {m}");
        assert!(try_op(Op::Filter { expr: "region > 1".into() }).contains("cannot compare"), "{e}");
        assert!(try_op(Op::Filter { expr: "price >".into() }).contains("ends too soon"), "{e}");
        assert!(try_op(Op::Filter { expr: "price".into() }).contains("true/false"), "{e}");
        assert!(try_op(Op::Sort { keys: vec![SortKey { column: "zzz".into(), dir: Dir::Asc }] }).contains("no column"), "{e}");
        assert!(try_op(Op::Select { columns: vec!["id".into(), "nope".into()] }).contains("no column"), "{e}");
        let bad = s.page(&PageRequest { pipeline: Pipeline::new(&DatasetHandle("d999".into())), offset: 0, limit: 1 }).unwrap_err();
        assert_eq!(bad, DataError::NoSuchDataset("d999".into()), "{e}");
        // a closed dataset is gone
        s.close(&i.handle);
        assert!(s.schema(&Pipeline::new(&i.handle)).is_err(), "{e}");
    }
}

#[test]
fn requests_and_results_are_json() {
    let req = PageRequest { pipeline: Pipeline::new(&DatasetHandle("d1".into())).with(Op::Filter { expr: "a > 1".into() }).with(Op::GroupBy { by: vec!["a".into()], aggs: vec![Agg { column: "b".into(), func: AggFn::Sum, alias: Some("t".into()) }] }), offset: 5, limit: 20 };
    let text = serde_json::to_string(&req).unwrap();
    assert!(text.contains("\"handle\":\"d1\"") && text.contains("\"op\":\"filter\"") && text.contains("\"as\":\"t\""), "{text}");
    let back: PageRequest = serde_json::from_str(&text).unwrap();
    assert_eq!(back, req);
    // a hand-written request with the defaults left out
    let hand: PageRequest = serde_json::from_value(json!({"handle": "d2", "limit": 10, "ops": [{"op": "sort", "keys": [{"column": "x", "dir": "desc"}]}, {"op": "limit", "n": 3}]})).unwrap();
    assert_eq!((hand.offset, hand.pipeline.ops.len()), (0, 2));
    let oreq: OpenRequest = serde_json::from_value(json!({"path": "/x/y.csv"})).unwrap();
    assert!(oreq.header && oreq.format.is_none());
    assert_eq!(Format::from_path("A.TSV"), Some(Format::Tsv));
    assert_eq!(Format::from_path("x.png"), None);
    // the service works through the JSON forms too
    let d = dir("json");
    let f = write(&d, "s.csv", SALES);
    for s in engines() {
        let i = open(&*s, &f);
        let req: PageRequest = serde_json::from_value(json!({"handle": i.handle.0, "limit": 2, "ops": [{"op": "filter", "expr": "price > 2.5"}]})).unwrap();
        let p = s.page(&req).unwrap();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["total"], 3, "{}", s.engine());
        assert_eq!(v["rows"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn a_header_less_file_and_ragged_rows() {
    let d = dir("ragged");
    let f = write(&d, "n.csv", "1,a\n2,b\n3,c\n");
    for s in engines() {
        let i = s.open(&OpenRequest { path: f.clone(), format: None, delimiter: None, header: false }).unwrap();
        assert_eq!(i.rows, 3, "{}", s.engine());
        assert_eq!(i.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["column_1", "column_2"], "{}", s.engine());
    }
    let dup = write(&d, "d.csv", "a,a,\n1,2,3\n");
    let s = crate::builtin_service();
    let i = s.open(&OpenRequest::new(dup)).unwrap();
    assert_eq!(i.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["a", "a_2", "column_3"]);
}

#[test]
fn two_hundred_thousand_rows_page_quickly() {
    let d = dir("big");
    let mut text = String::from("id,name,group,score,note\n");
    for i in 0..200_000u32 {
        // a deterministic scatter so a sort has real work to do
        let score = (i as u64 * 2_654_435_761 % 100_000) as f64 / 100.0;
        text.push_str(&format!("{i},user{i},g{},{score},\"note, {i}\"\n", i % 50));
    }
    let f = write(&d, "big.csv", &text);
    // an unoptimised build of Polars is an order of magnitude slower; the limits are for the release build
    let slack: u128 = if cfg!(debug_assertions) { 12 } else { 1 };
    for s in engines() {
        let e = s.engine();
        let t = Instant::now();
        let i = open(&*s, &f);
        assert_eq!(i.rows, 200_000, "{e}");
        let open_time = t.elapsed();
        // pages of the plain file
        let t = Instant::now();
        for off in [0u64, 100_000, 199_950] {
            let p = page(&*s, Pipeline::new(&i.handle), off, 50);
            assert_eq!(p.rows.len(), 50.min(200_000 - off as usize));
            assert_eq!(p.rows[0][0], json!(off));
        }
        let plain = t.elapsed();
        assert!(plain.as_millis() < 1500 * slack, "{e}: three plain pages took {plain:?}");
        // sorted: the first page does the work, the others come from the kept result
        let sorted = Pipeline::new(&i.handle).with(Op::Sort { keys: vec![SortKey { column: "score".into(), dir: Dir::Desc }] });
        let t = Instant::now();
        let first = page(&*s, sorted.clone(), 0, 50);
        let first_time = t.elapsed();
        let scores: Vec<f64> = first.rows.iter().map(|r| r[3].as_f64().unwrap()).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{e}: sorted descending");
        let t = Instant::now();
        for off in [50u64, 100_000, 199_950] {
            let p = page(&*s, sorted.clone(), off, 50);
            assert_eq!(p.total, 200_000);
            assert!(!p.rows.is_empty());
        }
        let later = t.elapsed();
        assert!(later.as_millis() < 1500 * slack, "{e}: later pages of a sort took {later:?}");
        // a filter shrinks the total
        let f1 = Pipeline::new(&i.handle).with(Op::Filter { expr: "group = 'g7'".into() });
        let p = page(&*s, f1, 0, 20);
        assert_eq!(p.total, 4_000, "{e}");
        eprintln!("{e}: open {open_time:?}, plain pages {plain:?}, first sort {first_time:?}, later sorted pages {later:?}");
    }
}

#[test]
fn json_lines_need_the_polars_engine_and_the_builtin_one_says_so() {
    let d = dir("jsonl");
    let f = write(&d, "e.jsonl", "{\"a\": 1, \"b\": \"x\"}\n{\"a\": 2, \"b\": null}\n{\"a\": 3, \"b\": \"z\"}\n");
    let b = crate::builtin_service().open(&OpenRequest::new(&f)).unwrap_err().to_string();
    assert!(b.contains("JSON lines") && b.contains("Convert"), "{b}");
    #[cfg(feature = "polars")]
    {
        let s = crate::polars_engine::PolarsEngine::new();
        let i = s.open(&OpenRequest::new(&f)).unwrap();
        assert_eq!(i.rows, 3);
        let p = page(&s, Pipeline::new(&i.handle).with(Op::Filter { expr: "a >= 2".into() }), 0, 10);
        assert_eq!(col(&p, "b"), vec![Value::Null, json!("z")]);
    }
}
