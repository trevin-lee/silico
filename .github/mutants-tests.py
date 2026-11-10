"""The tests a mutants run gives each mutant when its `tests` input is empty (mutants.yml's plan).

    python3 -I .github/mutants-tests.py METADATA MUTANTS

METADATA is `cargo metadata --no-deps --format-version 1` of the tree under test, and MUTANTS
its mutants as `cargo mutants --list` names them ("path:line:col: what was replaced"). Prints
two lines: the packages whose tests to run (each a --test-package), or nothing for every
workspace test; and why.

Only a package that depends on a mutated one, by any kind of dependency and at any depth,
can see a mutant through Cargo, so the mutated packages and those are its tests. That holds
for code tests reach through the libraries they build, not for a binary another package's
tests run by finding it on disk: the worker binaries, which silico-core's worker client
starts beside its own executable without depending on the packages that build them. A
mutant in code linked into a worker binary (a package that depends on silico-worker, or
silico-worker itself, and every package they depend on to build) is left to every workspace
test, which rebuilds every binary.
"""

import json
import os
import sys

WORKER = "silico-worker"


def main(metadata_path, mutants_path):
    with open(metadata_path) as f:
        metadata = json.load(f)
    root = metadata["workspace_root"]
    ids = set(metadata["workspace_members"])
    members = {p["name"]: p for p in metadata["packages"] if p["id"] in ids}
    # Each member's directory from the root, longest first, so a nested package wins.
    dirs = sorted(
        (
            (os.path.relpath(os.path.dirname(p["manifest_path"]), root), name)
            for name, p in members.items()
        ),
        key=lambda pair: -len(pair[0]),
    )
    mutated = set()
    with open(mutants_path) as f:
        for line in f:
            path = line.split(":", 1)[0].strip()
            if not path:
                continue
            for directory, name in dirs:
                if directory == "." or path == directory or path.startswith(directory + "/"):
                    mutated.add(name)
                    break
            else:
                return "", f"a mutant in {path} is in no package of the workspace: every workspace test"
    if not mutated:
        return "", "no mutants"
    # The workspace's own dependencies, each with its kind (None: normal, "dev", or "build").
    deps = {
        name: [
            (d["name"], d.get("kind"))
            for d in p["dependencies"]
            if d["name"] in members and d["name"] != name
        ]
        for name, p in members.items()
    }

    def linked(start):
        """start and every package it needs to build, by normal dependencies."""
        seen, stack = {start}, [start]
        while stack:
            for dep, kind in deps[stack.pop()]:
                if kind is None and dep not in seen:
                    seen.add(dep)
                    stack.append(dep)
        return seen

    workers = {
        name
        for name in members
        if name == WORKER or any(dep == WORKER and kind is None for dep, kind in deps[name])
    }
    in_workers = set().union(*(linked(w) for w in workers)) if workers else set()
    hit = sorted(mutated & in_workers)
    if hit:
        verb = "is" if len(hit) == 1 else "are"
        return "", (
            f"{' '.join(hit)} {verb} linked into the worker binaries, which other packages' "
            "tests run from disk: every workspace test"
        )
    dependents = {name: set() for name in members}
    for name in members:
        for dep, _ in deps[name]:
            dependents[dep].add(name)
    reach, stack = set(mutated), list(mutated)
    while stack:
        for dependent in dependents[stack.pop()]:
            if dependent not in reach:
                reach.add(dependent)
                stack.append(dependent)
    return " ".join(sorted(reach)), (
        f"the mutated packages ({' '.join(sorted(mutated))}) and those that depend on them"
    )


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: python3 -I .github/mutants-tests.py METADATA MUTANTS")
    tests, why = main(sys.argv[1], sys.argv[2])
    print(tests)
    print(why)
