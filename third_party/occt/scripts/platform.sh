# Sourced by build.sh, cache.sh, check-relocatable.sh, and the guard tests: commands that
# differ between macOS and Linux and that more than one script needs. A difference only
# one script cares about (build.sh's linker, check-math-imports.sh's symbol reader) stays
# in that script.
#   sha256_of [file]...        sha256 of the files, or of stdin
#   rpaths_of <lib>            the directories <lib> searches for its dependencies, one per
#                              line; fails if <lib> isn't a library
#   set_rpaths <lib> <dir>...  replace them
#   own_dir_rpath              the entry meaning "the library's own directory"
#   soname_flag <name>         link flag giving a library the name it's loaded by, so the
#                              path it was built at isn't recorded in it
#   lib_ext                    a shared library's file extension
#   minutes_ago <n>            a time <n> minutes ago, for touch -t

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

case "$(uname -s)" in
  Darwin)
    # shellcheck disable=SC2034
    own_dir_rpath=@loader_path lib_ext=dylib
    rpaths_of() {
      local cmds
      cmds=$(xcrun otool -l "$1") || return 1
      # otool exits 0 on files it can't parse, so require a library's identity.
      grep -q 'cmd LC_ID_DYLIB' <<< "$cmds" || return 1
      awk '/cmd LC_RPATH/ {getline; getline; print $2}' <<< "$cmds"
    }
    set_rpaths() {
      local lib=$1 rp
      shift
      for rp in $(rpaths_of "$lib"); do
        # (The "invalidates the code signature" warning is expected: the library is re-signed below.)
        xcrun install_name_tool -delete_rpath "$rp" "$lib" 2>/dev/null
      done
      for rp in "$@"; do xcrun install_name_tool -add_rpath "$rp" "$lib" 2>/dev/null; done
      /usr/bin/codesign --force --sign - "$lib" 2>/dev/null
    }
    soname_flag() { echo "-Wl,-install_name,@rpath/$1"; }
    minutes_ago() { date -r "$(($(date +%s) - $1 * 60))" +%Y%m%d%H%M; }
    ;;
  *)
    # shellcheck disable=SC2034,SC2016
    own_dir_rpath='$ORIGIN' lib_ext=so
    rpaths_of() { patchelf --print-rpath "$1" | tr ':' '\n'; }
    set_rpaths() {
      local lib=$1
      shift
      local IFS=:
      patchelf --set-rpath "$*" "$lib"
    }
    soname_flag() { echo "-Wl,-soname,$1"; }
    minutes_ago() { date -d "@$(($(date +%s) - $1 * 60))" +%Y%m%d%H%M; }
    ;;
esac
