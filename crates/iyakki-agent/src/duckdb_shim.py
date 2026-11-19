#!/usr/bin/env python3
# IYAKKI's `duckdb` command: a small stand-in for DuckDB's command-line program, run with the DuckDB Python package
# (the system's own DuckDB command is built for glibc and does not run on musl systems such as the IYAKKI images).
# Usage: duckdb [options] [file.db] [SQL ...]      duckdb -c "SELECT 1" -json      echo "SELECT 1;" | duckdb
# Options: -c SQL (repeatable), -json -csv -list -markdown -table -line, -header/-noheader, -separator S, -readonly,
#          -batch -no-stdin -init FILE -cmd SQL -echo -unsigned (accepted; -init and -cmd are ignored), -version
import csv
import decimal
import json
import os
import sys


def musl():
    return any(os.path.exists(p) for p in ("/lib/ld-musl-x86_64.so.1", "/lib/ld-musl-aarch64.so.1"))


def fail(msg, code=1):
    sys.stderr.write("Error: %s\n" % msg)
    sys.exit(code)


def cell(v):
    if v is None:
        return None
    if isinstance(v, (bool, int, float, str)):
        return v
    if isinstance(v, decimal.Decimal):
        return float(v)
    if isinstance(v, (bytes, bytearray)):
        return v.hex()
    return str(v)


def text(v, null):
    return null if v is None else str(v)


def show(cols, rows, mode, header, sep, null, out):
    if mode == "json":
        objs = [dict((c, cell(v)) for c, v in zip(cols, r)) for r in rows]
        if objs:
            out.write("[" + ",\n".join(json.dumps(o, ensure_ascii=False) for o in objs) + "]\n")
        else:
            out.write("[]\n")
    elif mode == "csv":
        w = csv.writer(out, lineterminator="\n")
        if header:
            w.writerow(cols)
        for r in rows:
            w.writerow([text(cell(v), null) for v in r])
    elif mode == "line":
        width = max([len(c) for c in cols] or [0])
        for i, r in enumerate(rows):
            if i:
                out.write("\n")
            for c, v in zip(cols, r):
                out.write("%s = %s\n" % (c.rjust(width), text(cell(v), null)))
    elif mode in ("markdown", "table"):
        body = [[text(cell(v), null) for v in r] for r in rows]
        widths = [max([len(c)] + [len(b[i]) for b in body]) for i, c in enumerate(cols)]
        line = lambda cells: "| " + " | ".join(x.ljust(widths[i]) for i, x in enumerate(cells)) + " |\n"
        out.write(line(cols))
        out.write("|" + "|".join("-" * (w + 2) for w in widths) + "|\n")
        for b in body:
            out.write(line(b))
    else:
        if header:
            out.write(sep.join(cols) + "\n")
        for r in rows:
            out.write(sep.join(text(cell(v), null) for v in r) + "\n")


def main(argv):
    try:
        import duckdb
    except Exception as e:  # noqa: BLE001
        fail("the DuckDB Python package cannot be loaded (%s)" % e)
    mode, header, sep, null, readonly, nostdin = "list", True, "|", "", False, False
    sqls, positional = [], []
    i = 0
    while i < len(argv):
        a = argv[i]
        i += 1
        if a in ("-version", "--version"):
            note = " (the newest DuckDB wheel built for musl)" if musl() else ""
            print("v%s via Python 3%s" % (duckdb.__version__, note))
            return 0
        if a in ("-json", "-csv", "-list", "-markdown", "-table", "-line"):
            mode = a[1:]
        elif a == "-header":
            header = True
        elif a == "-noheader":
            header = False
        elif a == "-readonly":
            readonly = True
        elif a in ("-batch", "-no-stdin", "-echo", "-unsigned", "-interactive", "-bail"):
            nostdin = nostdin or a == "-no-stdin"
        elif a in ("-c", "-s"):
            if i >= len(argv):
                fail("%s needs an argument" % a)
            sqls.append(argv[i])
            i += 1
        elif a in ("-init", "-cmd"):
            i += 1
        elif a == "-separator":
            if i >= len(argv):
                fail("-separator needs an argument")
            sep = argv[i]
            i += 1
        elif a == "-nullvalue":
            if i >= len(argv):
                fail("-nullvalue needs an argument")
            null = argv[i]
            i += 1
        elif a.startswith("-") and len(a) > 1:
            fail("unknown option: %s" % a)
        else:
            positional.append(a)
    db = positional[0] if positional else ":memory:"
    sqls += positional[1:]
    try:
        con = duckdb.connect(db, read_only=readonly) if db != ":memory:" else duckdb.connect()
    except Exception as e:  # noqa: BLE001
        fail(str(e))

    def run(sql):
        try:
            cur = con.execute(sql)
            if cur.description:
                cols = [d[0] for d in cur.description]
                show(cols, cur.fetchall(), mode, header, sep, null, sys.stdout)
        except Exception as e:  # noqa: BLE001
            fail(str(e))

    if sqls:
        for s in sqls:
            run(s)
        return 0
    if not nostdin and not sys.stdin.isatty():
        data = sys.stdin.read()
        if data.strip():
            run(data)
        return 0
    if nostdin:
        return 0
    print("v%s via Python 3 (IYAKKI)  Enter \".help\" for usage hints." % duckdb.__version__)
    buf = ""
    while True:
        try:
            line = input("D " if not buf else "> ")
        except EOFError:
            print()
            return 0
        if not buf and line.strip().startswith("."):
            cmd = line.strip().split()
            if cmd[0] in (".quit", ".exit", ".q"):
                return 0
            if cmd[0] == ".tables":
                run("SELECT table_name FROM information_schema.tables WHERE table_schema NOT IN ('information_schema','pg_catalog') ORDER BY 1")
            elif cmd[0] == ".help":
                print(".quit  leave\n.tables  list the tables\nEnd an SQL statement with ;")
            else:
                print("Unknown command or invalid arguments: \"%s\"." % cmd[0][1:])
            continue
        buf += line + "\n"
        if buf.rstrip().endswith(";"):
            run(buf)
            buf = ""


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
