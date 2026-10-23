# Sourced by CI scripts that call GitHub's CLI: puts gh on PATH. The runners' image hasn't
# it, so it's downloaded once per version into the runner's home, verified, and kept: the
# filler (.github/workflows/filler.yml) runs every 15 minutes. Several jobs may install it
# at once: each unpacks into its own directory and renames it into place, and the first
# rename wins.

GH_VERSION=2.102.1
GH_SHA256=77b9e297e94c50336823d0f84de7ef1be60cba704601fa0d98f129f47a94987b

if ! command -v gh >/dev/null 2>&1; then
  gh_dir=$HOME/.local/share/silico-ci/gh-$GH_VERSION
  if [ ! -x "$gh_dir/gh" ]; then
    mkdir -p "${gh_dir%/*}"
    gh_new=$(mktemp -d "$gh_dir.XXXXXX")
    curl -fsSL -o "$gh_new/gh.tar.gz" \
      "https://github.com/cli/cli/releases/download/v$GH_VERSION/gh_${GH_VERSION}_linux_amd64.tar.gz"
    echo "$GH_SHA256  $gh_new/gh.tar.gz" | sha256sum -c --quiet
    tar zxf "$gh_new/gh.tar.gz" -C "$gh_new" --strip-components 2 "gh_${GH_VERSION}_linux_amd64/bin/gh"
    rm "$gh_new/gh.tar.gz"
    mv -T "$gh_new" "$gh_dir" 2>/dev/null || rm -rf "$gh_new"
  fi
  PATH="$gh_dir:$PATH"
fi
