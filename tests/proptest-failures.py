"""The failed tests of a proptest exploration round's JUnit report (tests/proptest-explore.sh),
sorted and written as issues for .github/proptest-report.sh to open or comment on.

    python3 -I tests/proptest-failures.py list ROUND.xml
        One line for each failed test: its kind ("property" when its output gives proptest's
        shrunk input, "other" when not: a timeout, a crash, or a test that isn't a property's),
        its binary ID, its name, and cargo's arguments for it, tab-separated.
    python3 -I tests/proptest-failures.py failed-again RERUN.xml BINARY TEST
        Exits 0 if TEST, run again alone, failed again giving an input; 1 if not.
    python3 -I tests/proptest-failures.py render ROUND.xml SEED COMMIT OUTPUT RERUNS
        The issues, in OUTPUT/failures/<name>/title and body.md, and a line for each failed
        test in OUTPUT/failures.md. RERUNS holds a line for each property failure run again
        alone: its binary ID, its name, and what that run found (again: failed again, giving an
        input; passed; no-input: failed with none, a timeout, as load can make; unrun: out of
        time), tab-separated. RUN_URL, if set, is the run's link.

An issue of its own, titled "Proptest exploration failed: <binary> <test>" so a later failure
of the same test finds it, only for a property that failed again alone, and for at most
MOST_ISSUES of them: a run with more property failures than that reports them all in the one
issue the rest go to, "Proptest exploration failed: unconfirmed", as a starved or broken run
would fail hundreds. That one lists every other failure, each a line, so a run makes at most
MOST_ISSUES + 1 issues.
"""

import os
import sys
import xml.etree.ElementTree as ET

# GitHub refuses an issue body past 65,536 characters; the output is cut well inside that.
MOST_LINES = 120
MOST_CHARS = 20_000
# The most issues of their own a run opens or comments on; and the most characters of
# failures the unconfirmed issue lists, a line each (a broken runner fails every test).
MOST_ISSUES = 5
MOST_LISTED_CHARS = 40_000
AGGREGATE = "Proptest exploration failed: unconfirmed"


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


def failures(report):
    """Each failed test of a JUnit report, in its order: (binary ID, name, nextest's word for
    how it failed, its first attempt's output)."""
    found = []
    for case in ET.parse(report).getroot().iter("testcase"):
        failure = case.find("failure")
        if failure is None:
            failure = case.find("error")
        if failure is None:
            continue
        output = "\n".join(
            part.text or "" for part in (case.find("system-out"), case.find("system-err")) if part is not None
        )
        how = failure.get("type") or failure.get("message") or "failed"
        found.append((case.get("classname", ""), case.get("name", ""), how, output))
    return found


def shrunk(output):
    """Proptest's failure and its shrunk input from a test's output, and what to call it; or
    (None, None)."""
    found = proptest_input(output)
    if found is not None:
        return found, "Proptest's shrunk minimal input, and why it failed:"
    found = unwrapped_failure(output)
    if found is not None:
        return found, "Proptest's failure, its shrunk input last:"
    return None, None


def command(seed, binary_id, test):
    crate, flag = selector(binary_id)
    return f"PROPTEST_RNG_SEED={seed} pixi run cargo nextest run -p {crate} {flag} -E 'test(={test})'"


def write_issue(out, name, title, body):
    directory = os.path.join(out, "failures", name)
    os.makedirs(directory, exist_ok=True)
    with open(os.path.join(directory, "title"), "w", encoding="utf-8") as f:
        f.write(title + "\n")
    with open(os.path.join(directory, "body.md"), "w", encoding="utf-8") as f:
        f.write(body)


def list_failures(report):
    for binary_id, test, _, output in failures(report):
        kind = "property" if shrunk(output)[0] is not None else "other"
        crate, flag = selector(binary_id)
        print(f"{kind}\t{binary_id}\t{test}\t-p {crate} {flag}")


def failed_again(report, binary_id, test):
    for b, t, _, output in failures(report):
        if (b, t) == (binary_id, test) and shrunk(output)[0] is not None:
            return 0
    return 1


# What a property failure's run alone found, as the unconfirmed issue says it.
RERUN_WORDS = {
    "passed": "failed in the round, passed run again alone",
    "no-input": "failed in the round; run again alone, failed giving no input (a timeout?)",
    "unrun": "failed in the round, not run again alone: out of time",
}


def render(report, seed, commit, out, reruns_path):
    run_url = os.environ.get("RUN_URL", "")
    where = f": {run_url}." if run_url else "."
    reruns = {}
    with open(reruns_path, encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 3:
                reruns[(parts[0], parts[1])] = parts[2]
    failed = failures(report)
    properties = [f for f in failed if shrunk(f[3])[0] is not None]
    many = len(properties) > MOST_ISSUES

    own = []
    unconfirmed = []
    for binary_id, test, how, output in failed:
        found, what = shrunk(output)
        if found is None:
            unconfirmed.append((binary_id, test, f"{how}, giving no input"))
        elif many:
            unconfirmed.append((binary_id, test, f"{how}; one of {len(properties)} property failures"))
        elif reruns.get((binary_id, test)) == "again":
            own.append((binary_id, test, how, found, what))
        else:
            word = RERUN_WORDS.get(reruns.get((binary_id, test), "unrun"), "not run again alone")
            unconfirmed.append((binary_id, test, word))

    for n, (binary_id, test, how, found, what) in enumerate(own, start=1):
        # A fence longer than any run of backticks in what it holds.
        fence = "`" * max(3, longest_backticks(found) + 1)
        body = f"""`{test}` (`{binary_id}`) failed in a proptest exploration at seed {seed}, on {commit}{where} Nextest: {how}, after its retries; and failed again run alone at the seed.

{what}

{fence}text
{found}
{fence}

To run it again, at {commit}:

```sh
{command(seed, binary_id, test)}
```

Commit the case proptest saves when it fails there (an integration test's `.proptest-regressions` file beside it, or a library's under the crate's `proptest-regressions/`) with the fix, so every gate replays it. Close this once the fix is in: later explorations' seeds test other cases.
"""
        write_issue(out, str(n), f"Proptest exploration failed: {binary_id} {test}", body)

    if unconfirmed:
        lines = []
        size = 0
        for b, t, word in unconfirmed:
            line = f"- `{b}` `{t}`: {word}. `{command(seed, b, t)}`"
            if size + len(line) > MOST_LISTED_CHARS:
                lines.append(f"- and {len(unconfirmed) - len(lines)} more: the run's artifact has the round's report")
                break
            lines.append(line)
            size += len(line) + 1
        if many:
            why = (
                f"{len(properties)} tests failed giving proptest's input, more than the {MOST_ISSUES} a run"
                " reports one by one: likely the runner or the commit, not a property each. They"
                " weren't run again alone."
            )
        else:
            why = (
                "None of these failed giving proptest's input and then failed again alone at the"
                " seed: a timeout or a crash under the idle priority filler runs at, a test that"
                " isn't a property's, or a property only load makes fail."
            )
        body = f"""A proptest exploration at seed {seed}, on {commit}, had {len(unconfirmed)} failed tests this issue gathers{where}

{why}

{chr(10).join(lines)}

Each command runs one again at its seed. Every run with such failures comments here: close this once they pass, or are explained.
"""
        write_issue(out, "unconfirmed", AGGREGATE, body)

    with open(os.path.join(out, "failures.md"), "w", encoding="utf-8") as f:
        for binary_id, test, *_ in own:
            f.write(f"- `{binary_id}` `{test}`: failed again alone, an issue of its own. `{command(seed, binary_id, test)}`\n")
        for binary_id, test, word in unconfirmed:
            f.write(f"- `{binary_id}` `{test}`: {word}. `{command(seed, binary_id, test)}`\n")


def main():
    args = sys.argv[1:]
    if args[:1] == ["list"] and len(args) == 2:
        list_failures(args[1])
    elif args[:1] == ["failed-again"] and len(args) == 4:
        sys.exit(failed_again(*args[1:]))
    elif args[:1] == ["render"] and len(args) == 6:
        render(*args[1:])
    else:
        sys.exit(
            "usage: proptest-failures.py list ROUND.xml | failed-again RERUN.xml BINARY TEST"
            " | render ROUND.xml SEED COMMIT OUTPUT RERUNS"
        )


if __name__ == "__main__":
    main()
