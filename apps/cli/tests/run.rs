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
fn live_values_print_their_final_text() {
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
    let text = stdout(&output);
    assert!(text.contains("\"done\""), "{text}");
    assert!(!text.contains("starting"), "{text}");
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
fn usage_errors_exit_with_two() {
    let directory = tempdir().unwrap();
    let data = directory.path().join("data");
    let script = directory.path().join("notes.txt");
    fs::write(&script, "nothing to run\n").unwrap();

    let output = cpad(&data, &script);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains(".py, .cs or .sql"));
}
