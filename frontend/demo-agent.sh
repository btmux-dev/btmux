#!/bin/bash
# A scripted coding agent for docs-demos.ts.
#
#   exec -a <claude|codex|agy> /bin/bash demo-agent.sh <name>
#
# argv[0] makes btmux detect it as that agent; it reports its state through
# the pane's hook endpoint, like the real agents' hooks. BTMUX_DEMO picks the
# script (agents or latex), and it marks its progress as files in
# BTMUX_DEMO_SYNC for the recorder to wait on.
set -u
name="$1"

bold=$'\e[1m'
dim=$'\e[2m'
green=$'\e[32m'
yellow=$'\e[33m'
blue=$'\e[34m'
magenta=$'\e[35m'
reset=$'\e[0m'

pause() { sleep "$1"; }

hook() {
    curl --silent --output /dev/null --max-time 2 \
        --header "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
        --header "Content-Type: application/json" \
        --header "X-Btmux-Agent-Pid: $$" \
        --data "$1" "$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/notify"
}

mark() { [[ -n "${BTMUX_DEMO_SYNC:-}" ]] && touch "$BTMUX_DEMO_SYNC/$name-$1"; }

# Lines, then a pause.
say() {
    local delay="$1"
    shift
    printf '%s\n' "$@"
    pause "$delay"
}

step() { say "${2:-1}" "${blue}●${reset} $1"; }

prompt() { printf '\n%s>%s ' "$bold" "$reset"; }

permission() {
    printf '\n  %sAllow %s: %s?%s (y/n) ' "$yellow" "$1" "$2" "$reset"
    hook "{\"hook_event_name\":\"PermissionRequest\",\"tool_name\":\"$1\",\"tool_input\":{\"command\":\"$2\"}}"
    mark blocked
    local key=""
    while [[ "$key" != y ]]; do read -rsn1 key; done
    printf 'y\n\n'
    hook '{"hook_event_name":"PostToolUse"}'
}

done_turn() {
    hook "{\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"$1\"}"
    mark done
}

case "$name" in
claude) color="$magenta" ;;
codex) color="$green" ;;
*) color="$blue" ;;
esac
printf '%s%s%s %s(btmux demo agent)%s\n' "$bold$color" "$name" "$reset" "$dim" "$reset"
printf '%s~%s%s\n' "$dim" "${PWD#"$HOME"}" "$reset"
hook '{"hook_event_name":"SessionStart"}'
prompt
read -r _
hook '{"hook_event_name":"UserPromptSubmit"}'
echo

case "${BTMUX_DEMO:-agents}:$name" in
agents:claude)
    step "Read src/routes/orders.ts" 1.2
    step "Read src/server.ts" 1
    step "Edit src/routes/orders.ts ${green}+38${reset} ${dim}-4${reset}" 1.5
    step "Write tests/orders.test.ts ${green}+27${reset}" 1.2
    permission Bash "bun test orders"
    step "Bash bun test orders" 2
    say 1 "  ${dim}4 pass, 0 fail${reset}" ""
    say 0 "${bold}GET /orders${reset} now takes ${bold}?cursor=${reset} and ${bold}?limit=${reset} and returns a next cursor."
    done_turn "Paginated GET /orders with a cursor; all 4 order tests pass."
    ;;
agents:codex)
    step "Explored tests/server.test.ts" 2
    step "Ran bun test --rerun-each 50" 3
    say 2 "  ${dim}3 of 50 runs failed: address in use${reset}"
    step "Explored src/server.ts" 2
    say 0 "" "The tests share port 3000 with a server another test starts. Passing" \
        "PORT=0 picks a free port, so the runs no longer collide."
    done_turn "The tests race for port 3000; PORT=0 makes them pick free ports."
    ;;
agents:agy)
    step "List src/routes" 2
    step "Read src/routes/*.ts" 4
    step "Search \"pattern\"" 4
    step "Read src/server.ts" 5
    say 0 "Each route is a pattern and a handler; handle() tries them in order."
    done_turn "Each route is a pattern and a handler; handle() tries them in order."
    ;;
latex:*)
    step "Thinking" 1.5
    say 0.6 "Square the integral and switch to polar coordinates. Let"
    say 0.6 "[" "I = \int_{-\infty}^{\infty} e^{-x^2}\,dx" "]"
    say 0.6 "Then"
    say 0.6 "[" "I^2 = \int_{-\infty}^{\infty}\int_{-\infty}^{\infty} e^{-(x^2+y^2)}\,dx\,dy" "]"
    say 0.6 "With \(x = r\cos\theta\), \(y = r\sin\theta\) and \(dx\,dy = r\,dr\,d\theta\):"
    say 0.6 "[" "I^2 = \int_0^{2\pi}\int_0^{\infty} e^{-r^2}\,r\,dr\,d\theta = 2\pi \cdot \frac{1}{2} = \pi" "]"
    say 0.6 "Since \(I > 0\),"
    say 0 "[" "\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}" "]"
    done_turn "The Gaussian integral is the square root of pi."
    ;;
esac

while :; do
    prompt
    read -r _ || exit 0
done
