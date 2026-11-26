# Canban shell integration for fish (launched via `fish -l --init-command`).
if status --is-interactive; and not set -q __canban_hooks_loaded
    set -g __canban_hooks_loaded 1
    set -g __canban_cmd_active 0

    function __canban_osc
        printf '\e]7501;id=canban-shell:%s\e\\' "$argv[1]"
    end

    function __canban_b64
        printf '%s' "$argv[1]" | command base64 | command tr -d '\r\n'
    end

    function __canban_preexec --on-event fish_preexec
        set -g __canban_cmd_active 1
        set -l parts (string split ' ' -- (string trim -- "$argv[1]"))
        set -l first "$parts[1]"
        if test -n "$first"
            set -l base (string replace -r '.*/' '' -- "$first")
            set -l prog (string sub -l 32 -- "$base")
            set -l title_b64 (__canban_b64 "$prog")
            __canban_osc "state=working:app=shell:title=$title_b64"
        else
            __canban_osc "state=working:app=shell"
        end
    end

    function __canban_postexec --on-event fish_postexec
        set -l code $status
        if test "$__canban_cmd_active" != "1"
            return 0
        end
        set -g __canban_cmd_active 0
        set -l code_b64 (__canban_b64 "$code")
        if test "$code" -eq 0
            __canban_osc "state=done:app=shell:msg=$code_b64"
        else
            __canban_osc "state=error:app=shell:msg=$code_b64"
        end
    end
end
