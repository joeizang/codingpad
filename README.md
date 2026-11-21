# CodingPad

CodingPad is a desktop scratchpad for **Python, C#, TypeScript, Rust, and SQL**.
Write a short script, run it, and inspect the output beside your code. Use it
to try an idea, explore a database, or save a script you want to run again.

## Install CodingPad

Download the installer for your operating system from the
[latest release](https://github.com/joeizang/codingpad/releases/latest).

### macOS

Open the `_aarch64.dmg` on Apple silicon or the `_x64.dmg` on Intel and drag
**CodingPad** into **Applications**. If macOS blocks the first launch, see
the [first-launch steps for macOS](TECHNICAL_REFERENCE.md#macos).

You can also install with Homebrew:

```sh
brew tap joeizang/codingpad https://github.com/joeizang/codingpad.git
brew install --cask joeizang/codingpad/codingpad
```

### Windows

Run the `_x64-setup.exe` installer. If Windows shows a SmartScreen prompt, see
the [first-launch steps for Windows](TECHNICAL_REFERENCE.md#windows).

### Linux

Install the `.deb` on Debian or Ubuntu, or the `.rpm` on Fedora. For the
AppImage, make the downloaded file executable and open it. See the
[Linux install commands](TECHNICAL_REFERENCE.md#linux).

## Run your first example

1. Open **CodingPad** and find **Samples** in the sidebar.
2. Expand **SQL** and open **Orders and totals**. CodingPad opens an editable
   copy connected to the bundled SQLite database.
3. Click **Run**, or press **Ctrl+Enter** (**Cmd+Enter** on macOS). The **Results**
   pane shows a table of orders, customers, and totals.
4. Change `ORDER BY o.ordered_at` to `ORDER BY o.ordered_at DESC` and run again to
   show the newest orders first. Use **Export CSV** or **Export Excel** below
   the table if you want a copy of the results.

## Write and save a script

Click **+** and pick a language, or choose **Open file…** in the sidebar to
open an existing script.

| Language | What you need |
| --- | --- |
| Python (`.py`) | Python 3.10+ and `uv`. If `uv` is missing, choose **Install uv and run** when prompted. If Python is missing too, [install Python before rerunning](TECHNICAL_REFERENCE.md#first-launch). |
| C# (`.cs`) | .NET 10 or later. If prompted, choose **Install .NET and run**. |
| TypeScript (`.ts`) | Bun. If prompted, choose **Install Bun and run**. |
| Rust (`.rs`) | Cargo and rustc. If prompted, choose **Install Rust and run**. |
| SQL (`.sql`) | A saved connection. Choose **codingpad-sample** for the included database, or click **Manage…** in the sidebar’s **Connections** section to add SQLite, PostgreSQL, or SQL Server. |

In a Python tab, try this and click **Run**:

```python
print("Scores")
[{"name": "Ada", "score": 10}, {"name": "Grace", "score": 20}]
```

**Results** shows the printed text and displays the final expression as a table.
To inspect values elsewhere in a script, use `dump(value)` in Python,
`import { dump } from "codingpad"` and `dump(value)` in TypeScript, or
`value.Dump()` in C#. In Rust, call `codingpad::dump(value)`. Expand a result to
explore nested data, and click a line link in an error to jump to the code.

Press **Ctrl+S** (**Cmd+S** on macOS) to save your work as an ordinary source
file. The sidebar starts with your library in `Documents/CodingPad`, and you
can pin other folders to keep frequently used scripts on hand.

## Learn more

- Try more of the built-in **Samples** to see collections, queries, and richer output.
- See the reference for [database connections](TECHNICAL_REFERENCE.md#sql-scripts-m30),
  [secrets](TECHNICAL_REFERENCE.md#secrets), and
  [running scripts from the terminal](TECHNICAL_REFERENCE.md#run-from-the-terminal-m41).
- See the [technical reference](TECHNICAL_REFERENCE.md) for
  [building from source](TECHNICAL_REFERENCE.md#develop-locally), the repository
  layout, and release and development workflows.

CodingPad is dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
