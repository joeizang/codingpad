use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn cpad(data_dir: &Path, script: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpad"))
        .arg("run")
        .arg(script)
        .env("CODINGPAD_DATA_DIR", data_dir)
        .output()
        .expect("could not start cpad")
}

fn cpad_html(data_dir: &Path, script: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpad"))
        .arg("run")
        .arg("--html")
        .arg(script)
        .env("CODINGPAD_DATA_DIR", data_dir)
        .output()
        .expect("could not start cpad")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn python_script_prints_streams_and_dumps_as_text() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("hello.py");
    fs::write(
        &script,
        "print('hello from Python')\ndump({'name': 'Ada', 'score': 10})\n6 * 7\n",
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("hello from Python"), "{text}");
    assert!(text.contains("{name: \"Ada\", score: 10}"), "{text}");
    assert!(text.contains("\n42\n"), "{text}");
}

#[test]
fn live_values_stream_and_report_updates() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("live.py");
    fs::write(
        &script,
        "from codingpad import dump, live\n\
         status = live('starting')\n\
         dump(status)\n\
         status.value = 'done'\n",
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // The dump streams immediately; the update is progress on stderr, so a
    // terminal never has to hold back later output for a live value.
    assert!(
        stdout(&output).contains("\"starting\""),
        "{}",
        stdout(&output)
    );
    let errors = stderr(&output);
    assert!(errors.contains("live: \"done\""), "{errors}");
}

#[test]
fn live_updates_still_report_after_a_held_dump() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("live_after_lazy.py");
    fs::write(
        &script,
        "from codingpad import dump, live\nprogress = live(0)\ndump(progress)\n\
         value = 'leaf'\nfor _ in range(25):\n    value = {'next': value}\n\
         dump(value)\nprogress.value = 1\n",
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    // The live dump streamed before the deep dump forced later output to be
    // held; its update must still be reported instead of matching nothing.
    assert!(text.starts_with("0\n"), "{text}");
    assert!(text.contains("leaf"), "{text}");
    assert!(stderr(&output).contains("live: 1"), "{}", stderr(&output));
}

#[test]
fn cache_runs_a_factory_once_for_the_same_key() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("cache.py");
    fs::write(
        &script,
        concat!(
            "from codingpad import cache, dump\n",
            "calls = []\n",
            "def build():\n",
            "    calls.append(1)\n",
            "    return 42\n",
            "first = cache('answer', build)\n",
            "second = cache('answer', build)\n",
            "dump({'first': first, 'second': second, 'calls': len(calls)})\n",
        ),
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("calls: 1"), "{text}");
    assert!(text.contains("first: 42"), "{text}");
    assert!(text.contains("second: 42"), "{text}");
}

#[test]
fn html_output_is_one_page_with_escaped_streams_and_structured_dumps() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("report.py");
    fs::write(
        &script,
        concat!(
            "from codingpad import html\n",
            "print('hello <b>world</b>')\n",
            "print('y' * 20000)\n",
            "dump({'name': 'Ada', 'score': 10})\n",
            "dump([1, 2, 3])\n",
            "dump(html('<meta http-equiv=\"refresh\" content=\"0;url=https://example.com\"><b>ok</b>'))\n",
        ),
    )
    .unwrap();

    let output = cpad_html(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let page = stdout(&output);
    assert!(page.starts_with("<!doctype html>"), "{page}");
    assert!(page.ends_with("</html>\n"), "{page}");
    // Streams keep their text but never inject markup, and adjacent chunks
    // from one stream share a single block.
    assert!(page.contains("hello &lt;b&gt;world&lt;/b&gt;"), "{page}");
    assert!(!page.contains("hello <b>world</b>"), "{page}");
    assert_eq!(page.matches("class=\"stream stdout\"").count(), 1, "{page}");
    // Dumps keep their structure rather than becoming plain text.
    assert!(page.contains("<dl class=\"object\">"), "{page}");
    assert!(page.contains("<dt>name</dt>"), "{page}");
    assert!(page.contains("<ul class=\"list\">"), "{page}");
    // HTML dumps keep markup, but navigation tags cannot escape the report.
    assert!(page.contains("<b>ok</b>"), "{page}");
    assert!(!page.contains("http-equiv=\"refresh\""), "{page}");
    // The page is standalone: no script, form or network access is allowed.
    assert!(page.contains("default-src 'none'"), "{page}");
    assert!(page.contains("form-action 'none'"), "{page}");
    assert!(page.contains("base-uri 'none'"), "{page}");
}

#[test]
fn html_compile_errors_also_reach_the_terminal() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("broken.cs");
    fs::write(&script, "this is not C#;\n").unwrap();

    let output = cpad_html(&data, &script);
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    // The page keeps the diagnostics, and the terminal shows why the run failed.
    let errors = stderr(&output);
    assert!(errors.contains("CS"), "{errors}");
    assert!(stdout(&output).contains("CS"), "{}", stdout(&output));
}

#[test]
fn html_runtime_errors_also_reach_the_terminal() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("boom.py");
    fs::write(&script, "raise ValueError('boom')\n").unwrap();

    let output = cpad_html(&data, &script);
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    // A runtime error arrives as a dump, not stderr: mirror its summary so the
    // terminal explains the failure while the page keeps the full traceback.
    let errors = stderr(&output);
    assert!(errors.contains("ValueError: boom"), "{errors}");
    let page = stdout(&output);
    assert!(page.contains("ValueError"), "{page}");
    assert!(page.contains("boom"), "{page}");
}

#[test]
fn html_live_updates_replace_dumps_and_keep_streams_together() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("live_report.py");
    fs::write(
        &script,
        concat!(
            "from codingpad import dump, live\n",
            "status = live('starting')\n",
            "dump(status)\n",
            "for i in range(600):\n",
            "    print(i)\n",
            "    status.value = i\n",
        ),
    )
    .unwrap();

    let output = cpad_html(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let page = stdout(&output);
    // The page holds the final value, not the initial preview, even though the
    // text holder would have overflowed its 500-event cap long before.
    assert!(!page.contains("starting"), "{page}");
    assert!(page.contains("<span class=\"number\">599</span>"), "{page}");
    // Updates between prints must not split the surrounding stream: every
    // stdout read shares one block.
    assert_eq!(page.matches("class=\"stream stdout\"").count(), 1, "{page}");
}

#[test]
fn cache_keeps_keys_independent() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("cached.py");
    fs::write(
        &script,
        concat!(
            "from codingpad import cache\n",
            "calls = {}\n",
            "def make(key):\n",
            "    calls[key] = calls.get(key, 0) + 1\n",
            "    return key.upper()\n",
            "print(cache('a', lambda: make('a')))\n",
            "print(cache('b', lambda: make('b')))\n",
            "print(cache('a', lambda: make('a')))\n",
            "print(calls)\n",
        ),
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("A\nB\nA\n"), "{text}");
    assert!(text.contains("{'a': 1, 'b': 1}"), "{text}");
}

#[test]
fn failing_python_script_exits_with_one() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("broken.py");
    fs::write(&script, "print('before')\n1 / 0\n").unwrap();

    let output = cpad(&data, &script);
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("before"), "{text}");
    assert!(text.contains("ZeroDivisionError"), "{text}");
    assert!(text.contains("File \"<codingpad>\""), "{text}");
}

#[test]
fn csharp_script_prints_streams_and_dumps_as_text() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("hello.cs");
    fs::write(
        &script,
        "Console.WriteLine(\"hello from C#\");\nnew { Name = \"Ada\", Score = 10 }.Dump();\n",
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("hello from C#"), "{text}");
    assert!(text.contains("{Name: \"Ada\", Score: 10}"), "{text}");
}

#[test]
fn failing_csharp_script_exits_with_one() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("broken.cs");
    fs::write(&script, "throw new InvalidOperationException(\"boom\");\n").unwrap();

    let output = cpad(&data, &script);
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("InvalidOperationException"), "{text}");
    assert!(text.contains("boom"), "{text}");
}

#[test]
fn sql_script_runs_through_its_connection_and_prints_tables() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let database = directory.path().join("sample.db");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("connections.json"),
        serde_json::to_string(&json!([{
            "id": "sample-id",
            "name": "sample",
            "kind": "sqlite",
            "path": database.to_string_lossy(),
        }]))
        .unwrap(),
    )
    .unwrap();
    let script = directory.path().join("people.sql");
    fs::write(
        &script,
        "-- codingpad: connection=sample\n\
         CREATE TABLE people (name TEXT, score INTEGER);\n\
         INSERT INTO people VALUES ('Ada', 10), ('Grace', 20);\n\
         SELECT name, score FROM people ORDER BY name;\n",
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("name   score"), "{text}");
    assert!(text.contains("Ada    10"), "{text}");
    assert!(text.contains("Grace  20"), "{text}");
}

#[test]
fn connection_header_and_cache_work_like_the_app() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let database = directory.path().join("sample.db");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("connections.json"),
        serde_json::to_string(&json!([{
            "id": "sample-id",
            "name": "sample",
            "kind": "sqlite",
            "path": database.to_string_lossy(),
        }]))
        .unwrap(),
    )
    .unwrap();
    // Create the schema first: the generated models come from the schema that
    // exists when the run starts.
    let setup = directory.path().join("setup.sql");
    fs::write(
        &setup,
        "-- codingpad: connection=sample\n\
         CREATE TABLE people (name TEXT, score INTEGER);\n\
         INSERT INTO people VALUES ('Ada', 10), ('Grace', 20);\n",
    )
    .unwrap();
    let output = cpad(&data, &setup);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let script = directory.path().join("people.py");
    fs::write(
        &script,
        concat!(
            "# codingpad: connection=sample\n",
            "from sqlalchemy import select\n",
            "people = context.session.scalars(select(context.Person)).all()\n",
            "print([person.name for person in people])\n",
            "print(db.query('SELECT count(*) AS total FROM people'))\n",
            "first = cache('answer', lambda: object())\n",
            "second = cache('answer', lambda: object())\n",
            "print(first is second)\n",
        ),
    )
    .unwrap();

    let output = cpad(&data, &script);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("['Ada', 'Grace']"), "{text}");
    assert!(text.contains("{'total': 2}"), "{text}");
    assert!(text.contains("True"), "{text}");
}

#[test]
fn usage_errors_exit_with_two() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("notes.txt");
    fs::write(&script, "nothing to run\n").unwrap();

    let output = cpad(&data, &script);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains(".py, .cs or .sql"));

    // The type check comes before opening the file, so a missing unknown path
    // is still a usage error rather than "Could not open".
    let missing = directory.path().join("missing.txt");
    let output = cpad(&data, &missing);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains(".py, .cs or .sql"));

    // An unknown option is a usage error too.
    let output = Command::new(env!("CARGO_BIN_EXE_cpad"))
        .args(["run", "--nope", "hello.py"])
        .env("CODINGPAD_DATA_DIR", &data)
        .output()
        .expect("could not start cpad");
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("Unknown option"));
}
