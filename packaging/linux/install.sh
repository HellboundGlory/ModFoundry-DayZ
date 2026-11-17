#!/usr/bin/env bash
# Installs ModFoundry for the current user: the programs into ~/.local/lib/modfoundry,
# mf and modfoundry-mcp-server linked into ~/.local/bin, the modfoundry skill, and the
# MCP server registered with the agents you pick.
# Usage: install.sh [--allow-mutations] [--yes] [--target <t>]... | --uninstall
#   --yes      registers each agent found on PATH, plus the agents folder
#   --target   repeatable; one of the targets below, with no questions asked
#   targets:   claude-user (Claude Code, all projects), claude-project=<dir> (one
#              project folder), codex, command-code, agents (~/.agents), print
#              (the command line for any other MCP client)
# Without flags, a terminal asks a question per agent; otherwise the programs install
# and the command line is printed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
lib="$HOME/.local/lib/modfoundry"
bin="$HOME/.local/bin"
server="$bin/modfoundry-mcp-server"
programs=(mf modfoundry-mcp-server modfoundry-steam libsteam_api.so)
linked=(mf modfoundry-mcp-server)

allow=""
uninstall=0
yes=0
targets=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --allow-mutations) allow="--allow-mutations" ;;
        --uninstall) uninstall=1 ;;
        --yes) yes=1 ;;
        --target)
            [[ $# -ge 2 ]] || { echo "--target needs a value" >&2; exit 2; }
            targets+=("$2")
            shift
            ;;
        -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done
json_text() { # $1 as it reads inside a JSON string
    local text="${1//\\/\\\\}"
    printf '%s' "${text//\"/\\\"}"
}

server_line="$(printf '%q' "$server")${allow:+ $allow}"
json_server="$(json_text "$server")"

ours() { [[ -L "$bin/$1" && "$(readlink "$bin/$1")" == "$lib/$1" ]]; }

owns() { # true when an agent's entry runs our program as its command
    local linked_server=""
    # The bin path is ours only while it is our link; anything else there belongs to someone else.
    if ours modfoundry-mcp-server; then linked_server="$server"; fi
    local alt="$lib/modfoundry-mcp-server" json_ours=""
    [[ -z "$linked_server" ]] || json_ours="$(json_text "$linked_server")"
    # Paths go in through the environment: awk -v would read their backslashes as escapes.
    OURS="$linked_server" ALT="$alt" JSON_OURS="$json_ours" JSON_ALT="$(json_text "$alt")" awk '
        BEGIN { ours = ENVIRON["OURS"]; alt = ENVIRON["ALT"]; jours = ENVIRON["JSON_OURS"]; jalt = ENVIRON["JSON_ALT"] }
        /^[[:space:]]*Command:[[:space:]]/ {
            value = $0
            sub(/^[[:space:]]*Command:[[:space:]]*/, "", value)
            sub(/[[:space:]]+$/, "", value)
            if (value != "" && (value == ours || value == alt)) found = 1
        }
        match($0, /"command"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"/) {
            value = substr($0, RSTART, RLENGTH)
            sub(/^"command"[[:space:]]*:[[:space:]]*"/, "", value)
            value = substr(value, 1, length(value) - 1)
            if (value != "" && (value == jours || value == jalt)) found = 1
        }
        END { exit !found }
    ' <<<"$1"
}

# A real entry names a program or a URL; any other answer (a notice, an error) is no entry.
is_entry() { grep -q -E '^[[:space:]]*(Command|URL):|"(command|url)"' <<<"$1"; }

# claude mcp get shows the entry that wins (local, then project, then user), so check its scope.
in_scope() { grep -q "^[[:space:]]*Scope: $2 config" <<<"$1"; }

home_path() { # a leading ~ as the shell would expand it
    case "$1" in
        "~") echo "$HOME" ;;
        "~/"*) echo "$HOME/${1#"~/"}" ;;
        *) echo "$1" ;;
    esac
}

agents_entry() { # the modfoundry entry in an agents config as JSON, empty when none
    jq -c '.mcpServers.modfoundry // empty' "$1" 2>/dev/null
}

unregister_agents() {
    local entry=""
    if command -v claude >/dev/null; then
        entry="$(cd "$HOME" && claude mcp get modfoundry 2>/dev/null)" || entry=""
        if owns "$entry" && in_scope "$entry" User; then
            (cd "$HOME" && claude mcp remove -s user modfoundry >/dev/null 2>&1) || true
        fi
    fi
    if command -v codex >/dev/null; then
        entry="$(codex mcp get modfoundry --json 2>/dev/null)" || entry=""
        if owns "$entry"; then codex mcp remove modfoundry >/dev/null 2>&1 || true; fi
    fi
    if command -v command-code >/dev/null; then
        entry="$(command-code mcp get modfoundry 2>/dev/null)" || entry=""
        if owns "$entry"; then command-code mcp remove -s user modfoundry >/dev/null 2>&1 || true; fi
    fi
}

remove_agents_entry() {
    local file="$HOME/.agents/mcp.json" skill="$HOME/.agents/skills/modfoundry/SKILL.md" tmp
    if [[ -f "$file" ]] && ! command -v jq >/dev/null && grep -q '"modfoundry"' "$file"; then
        echo "jq not found; remove the modfoundry entry from mcpServers in $file by hand"
    elif [[ -f "$file" ]] && owns "$(agents_entry "$file")"; then
        tmp="$(mktemp "$file.XXXXXX")"
        jq 'del(.mcpServers.modfoundry)' "$file" > "$tmp" && cat "$tmp" > "$file"
        rm -f "$tmp"
    fi
    if cmp -s "$skill" "$lib/skill/modfoundry/SKILL.md"; then
        rm -f "$skill"
        rmdir "$(dirname "$skill")" 2>/dev/null || true
    fi
}

if [[ $uninstall == 1 ]]; then
    unregister_agents
    remove_agents_entry
    for name in "${linked[@]}"; do
        if ours "$name"; then rm -f "$bin/$name"; fi
    done
    rm -rf "$lib"
    echo "removed; your data in ~/.local/share/modfoundry and ~/.cache/modfoundry is kept"
    exit 0
fi

for i in "${!targets[@]}"; do
    target="${targets[$i]}"
    case "$target" in
        claude-user | codex | command-code | agents | print) ;;
        claude-project=*)
            target="claude-project=$(home_path "${target#claude-project=}")"
            targets[i]="$target"
            [[ -d "${target#claude-project=}" ]] || {
                echo "not a folder: ${target#claude-project=}" >&2
                exit 2
            }
            ;;
        *) echo "unknown target: $target" >&2; exit 2 ;;
    esac
done

for name in "${linked[@]}"; do
    if [[ -e "$bin/$name" || -L "$bin/$name" ]] && ! ours "$name"; then
        echo "$bin/$name exists and is not ModFoundry's link; move it aside first" >&2
        exit 1
    fi
done

mkdir -p "$lib" "$bin"
for program in "${programs[@]}"; do
    install -m755 "$here/$program" "$lib/$program"
done
for name in "${linked[@]}"; do
    ln -sfn "$lib/$name" "$bin/$name"
done
prev_skill_sum=""
if [[ -f "$lib/skill/modfoundry/SKILL.md" ]]; then prev_skill_sum="$(sha256sum < "$lib/skill/modfoundry/SKILL.md")"; fi
mkdir -p "$lib/skill/modfoundry"
install -m644 "$here/skill/modfoundry/SKILL.md" "$lib/skill/modfoundry/SKILL.md"
echo "installed by install.sh" > "$lib/.modfoundry-install"

skip_missing() { # agent name, command to run by hand
    echo "$1 not found; skipped. To register it, run: $2"
}

may_add() { # agent name, entry text (empty when the agent holds none)
    if [[ -z "$2" ]] || ! is_entry "$2" || owns "$2"; then return 0; fi
    echo "modfoundry is already registered with $1 for another program; left as it is"
    return 1
}

register_claude() { # scope, folder the claude commands run in
    local scope="$1" folder="$2" entry=""
    if ! command -v claude >/dev/null; then
        local run="claude mcp add -s $scope modfoundry -- $server_line"
        [[ $scope == user ]] || run="cd $(printf '%q' "$folder") && $run"
        skip_missing claude "$run"
        return 0
    fi
    entry="$(cd "$folder" && claude mcp get modfoundry 2>/dev/null)" || entry=""
    may_add claude "$entry" || return 0
    if owns "$entry" && in_scope "$entry" "${scope^}"; then
        (cd "$folder" && claude mcp remove -s "$scope" modfoundry >/dev/null 2>&1) || true
    fi
    # shellcheck disable=SC2086
    (cd "$folder" && claude mcp add -s "$scope" modfoundry -- "$server" $allow) ||
        echo "claude mcp add failed; run it again later" >&2
}

register_codex() {
    local entry=""
    if ! command -v codex >/dev/null; then
        skip_missing codex "codex mcp add modfoundry -- $server_line"
        return 0
    fi
    entry="$(codex mcp get modfoundry --json 2>/dev/null)" || entry=""
    may_add codex "$entry" || return 0
    if owns "$entry"; then codex mcp remove modfoundry >/dev/null 2>&1 || true; fi
    # shellcheck disable=SC2086
    codex mcp add modfoundry -- "$server" $allow ||
        echo "codex mcp add failed; run it again later" >&2
}

register_command_code() {
    local entry=""
    if ! command -v command-code >/dev/null; then
        skip_missing command-code \
            "command-code mcp add --transport stdio -s user modfoundry -- $server_line"
        return 0
    fi
    entry="$(command-code mcp get modfoundry 2>/dev/null)" || entry=""
    may_add command-code "$entry" || return 0
    if owns "$entry"; then command-code mcp remove -s user modfoundry >/dev/null 2>&1 || true; fi
    # shellcheck disable=SC2086
    command-code mcp add --transport stdio -s user modfoundry -- "$server" $allow ||
        echo "command-code mcp add failed; run it again later" >&2
}

register_agents() {
    local dir="$HOME/.agents" args="[]" current tmp
    local file="$dir/mcp.json" skill="$dir/skills/modfoundry/SKILL.md"
    [[ -z "$allow" ]] || args='["--allow-mutations"]'
    mkdir -p "$dir/skills/modfoundry"
    if [[ ! -e "$skill" ]] || cmp -s "$skill" "$lib/skill/modfoundry/SKILL.md" ||
        [[ "$(sha256sum < "$skill")" == "$prev_skill_sum" ]]; then
        install -m644 "$lib/skill/modfoundry/SKILL.md" "$skill"
    else
        echo "$skill is not ModFoundry's; left as it is"
    fi
    if [[ ! -f "$file" || -z "$(tr -d '[:space:]' < "$file")" ]]; then
        printf '{"mcpServers": {"modfoundry": {"command": "%s", "args": %s}}}\n' "$json_server" "$args" > "$file"
        return 0
    fi
    if ! command -v jq >/dev/null; then
        echo "jq not found; add this entry to mcpServers in $file by hand:"
        echo "  \"modfoundry\": {\"command\": \"$json_server\", \"args\": $args}"
        return 0
    fi
    current="$(agents_entry "$file")" || {
        echo "$file is not valid JSON; left as it is" >&2
        return 0
    }
    may_add agents "$current" || return 0
    tmp="$(mktemp "$file.XXXXXX")"
    jq --arg command "$server" --argjson args "$args" \
        '.mcpServers.modfoundry = {command: $command, args: $args}' "$file" > "$tmp"
    if [[ -s "$tmp" ]]; then cat "$tmp" > "$file"; fi
    rm -f "$tmp"
}

default_targets() {
    command -v claude >/dev/null && echo claude-user
    command -v codex >/dev/null && echo codex
    command -v command-code >/dev/null && echo command-code
    echo agents
}

ask() { # question; true when the answer starts with y
    local reply=""
    read -r -p "$1 [y/N] " reply || true
    [[ "$reply" =~ ^[Yy] ]]
}

ask_claude() {
    local reply="" folder=""
    read -r -p "Register ModFoundry with Claude Code: [a]ll projects, [o]ne project, [n]one? [n] " reply || true
    case "$reply" in
        a | A) targets+=(claude-user) ;;
        o | O)
            read -e -r -p "Project folder: " folder || true
            folder="$(home_path "$folder")"
            if [[ -d "$folder" ]]; then
                targets+=("claude-project=$folder")
            else
                echo "not a folder: $folder; skipped" >&2
            fi
            ;;
    esac
}

ask_questions() {
    if command -v claude >/dev/null; then ask_claude; fi
    if command -v codex >/dev/null && ask "Register ModFoundry with codex?"; then targets+=(codex); fi
    if command -v command-code >/dev/null && ask "Register ModFoundry with command-code?"; then
        targets+=(command-code)
    fi
    if ask "Register ModFoundry with the shared ~/.agents folder?"; then targets+=(agents); fi
    if ask "Print the MCP command line for another client?"; then targets+=(print); fi
}

if [[ ${#targets[@]} -eq 0 ]]; then
    if [[ $yes == 1 ]]; then
        mapfile -t targets < <(default_targets)
    elif [[ -t 0 ]]; then
        ask_questions
    else
        targets=(print)
    fi
fi

for target in "${targets[@]}"; do
    case "$target" in
        agents) register_agents ;;
        print) echo "$server_line" ;;
        claude-user) register_claude user "$HOME" ;;
        claude-project=*) register_claude local "${target#claude-project=}" ;;
        codex) register_codex ;;
        command-code) register_command_code ;;
    esac
done

case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "add $bin to your PATH to run mf" ;;
esac
"$lib/mf" --version
