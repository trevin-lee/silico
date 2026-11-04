"""Each failed test in a proptest exploration round's JUnit report (tests/proptest-explore.sh),
as an issue's title and body for .github/proptest-report.sh to open or comment on.

    python3 -I tests/proptest-failures.py ROUND.xml SEED COMMIT OUTPUT

Writes OUTPUT/failures/N/title and OUTPUT/failures/N/body.md, one N for each failed test in the
report's order, and OUTPUT/failures.md, a line for each. The title names the test by its binary
and name, so a later failure of the same test finds its issue. The body gives the seed, the
commit, proptest's shrunk minimal input (from the first attempt's output, which nextest keeps
for a failed test), and the command that runs it again. RUN_URL, if set, is the run's link.
"""

import os
import sys
import xml.etree.ElementTree as ET

# GitHub refuses an issue body past 65,536 characters; the output is cut well inside that.
MOST_LINES = 120
MOST_CHARS = 20_000


def cut(lines):
    """At most MOST_LINES lines and MOST_CHARS characters, saying what was cut."""
    text = "\n".join(lines[:MOST_LINES])
    cut_lines = len(lines) > MOST_LINES
    if len(text) > MOST_CHARS:
        text = text[:MOST_CHARS]
        cut_lines = True
    return text + ("\n[... cut]" if cut_lines else "")


def proptest_input(output):
    """Proptest's failure, as it panics with it: "Test failed: <why>." and "minimal failing
    input: <value>", up to the counts it gives after ("\tsuccesses: ..."). The last one: a body
    that panics prints a panic for every case shrinking tried before it. None if there's none."""
    lines = output.splitlines()
    starts = [i for i, line in enumerate(lines) if line.lstrip().startswith("Test failed: ")]
    if not starts:
        return None
    start = starts[-1]
    inputs = [i for i in range(start, len(lines)) if "minimal failing input: " in lines[i]]
    if not inputs:
        return None
    end = len(lines)
    for i in range(inputs[0], len(lines)):
        if lines[i].startswith("\tsuccesses: "):
            end = i
            break
    return cut(lines[start:end])


def unwrapped_failure(output):
    """A TestRunner's result unwrapped: "called `Result::unwrap()` on an `Err` value: Fail(...)",
    proptest's error as Rust debug-prints it, its input last. None if there's none."""
    lines = output.splitlines()
    found = [i for i, line in enumerate(lines) if "`Err` value: Fail(" in line]
    if not found:
        return None
    start = found[-1]
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("note: run with `RUST_BACKTRACE") or lines[i].startswith("stack backtrace:"):
            end = i
            break
    return cut(lines[start:end])


def panic_tail(output):
    """The last panic and what follows it, or the output's last lines if nothing panicked."""
    lines = output.splitlines()
    panics = [i for i, line in enumerate(lines) if " panicked at " in line]
    tail = lines[panics[-1] :] if panics else lines[-40:]
    return cut(tail) if tail else "(no output)"


def longest_backticks(text):
    """The longest run of backticks in text."""
    longest = run = 0
    for c in text:
        run = run + 1 if c == "`" else 0
        longest = max(longest, run)
    return longest


def selector(binary_id):
    """The crate, and cargo's flag for the binary, from nextest's binary ID: "crate" for its
    library's unit tests, "crate::name" for an integration test, "crate::bin/name" and the like
    for the rest."""
    crate, _, binary = binary_id.partition("::")
    if not binary:
        return crate, "--lib"
    kind, slash, name = binary.partition("/")
    if slash and kind in ("bin", "bench", "example"):
        return crate, f"--{kind} {name}"
    return crate, f"--test {binary}"


def main():
    if len(sys.argv) != 5:
        sys.exit("usage: proptest-failures.py ROUND.xml SEED COMMIT OUTPUT")
    report, seed, commit, out = sys.argv[1:]
    run_url = os.environ.get("RUN_URL", "")
    failed = []
    for case in ET.parse(report).getroot().iter("testcase"):
        failure = case.find("failure")
        if failure is None:
            failure = case.find("error")
        if failure is None:
            continue
        failed.append((case, failure))

    summary = []
    for n, (case, failure) in enumerate(failed, start=1):
        binary_id = case.get("classname", "")
        test = case.get("name", "")
        output = "\n".join(
            part.text or "" for part in (case.find("system-out"), case.find("system-err")) if part is not None
        )
        crate, flag = selector(binary_id)
        how = failure.get("type") or failure.get("message") or "failed"
        found = proptest_input(output)
        if found is not None:
            what = "Proptest's shrunk minimal input, and why it failed:"
        else:
            found = unwrapped_failure(output)
            if found is not None:
                what = "Proptest's failure, its shrunk input last:"
        if found is not None:
            after = (
                "Commit the case proptest saves when it fails there (an integration test's"
                " `.proptest-regressions` file beside it, or a library's under the crate's"
                " `proptest-regressions/`) with the fix, so every gate replays it. Close this once"
                " the fix is in: later explorations' seeds test other cases."
            )
        else:
            found = panic_tail(output)
            what = (
                "Its output names no minimal failing input (a timeout, a crash, or a test that"
                " isn't a property's: the seed may not matter). Its last panic, or its output's end:"
            )
            after = "Close this once it's fixed."
        # A fence longer than any run of backticks in what it holds.
        fence = "`" * max(3, longest_backticks(found) + 1)
        command = f"PROPTEST_RNG_SEED={seed} pixi run cargo nextest run -p {crate} {flag} -E 'test(={test})'"
        title = f"Proptest exploration failed: {binary_id} {test}"
        where = f": {run_url}." if run_url else "."
        body = f"""`{test}` (`{binary_id}`) failed in a proptest exploration at seed {seed}, on {commit}{where} Nextest: {how}, after its retries.

{what}

{fence}text
{found}
{fence}

To run it again, at {commit}:

```sh
{command}
```

{after}
"""
        directory = os.path.join(out, "failures", str(n))
        os.makedirs(directory, exist_ok=True)
        with open(os.path.join(directory, "title"), "w", encoding="utf-8") as f:
            f.write(title + "\n")
        with open(os.path.join(directory, "body.md"), "w", encoding="utf-8") as f:
            f.write(body)
        summary.append(f"- `{binary_id}` `{test}`: `{command}`")

    with open(os.path.join(out, "failures.md"), "w", encoding="utf-8") as f:
        f.write("".join(line + "\n" for line in summary))


if __name__ == "__main__":
    main()
