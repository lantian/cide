# Sourced by Bash's ENV startup entry. Bash is interactive and a login shell, but POSIX mode
# must be off before any normal profile sees it. This script stays compatible with Bash 3.2.
set +o posix
_cide_bash_history_path=$_CIDE_BASH_HISTORY_FILE
export -n _cide_bash_history_path
if [ "$_CIDE_BASH_ENV_WAS_SET" = 1 ]; then
    export ENV=$_CIDE_BASH_ORIGINAL_ENV
else
    unset ENV
fi
unset _CIDE_BASH_INIT_FILE _CIDE_BASH_HISTORY_FILE _CIDE_BASH_ENV_WAS_SET _CIDE_BASH_ORIGINAL_ENV

# Select the private file even while profiles run (some profiles explicitly read history).
HISTFILE=$_cide_bash_history_path
if [ -r /etc/profile ]; then
    . /etc/profile
fi
for _cide_bash_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
    if [ -r "$_cide_bash_profile" ]; then
        . "$_cide_bash_profile"
        break
    fi
done
unset _cide_bash_profile

# Profiles may reset HISTFILE or read shared history. Discard that list; Bash automatically
# loads the selected pane file after this startup script returns, so do not history -r here.
builtin history -c
if ! builtin export "HISTFILE=$_cide_bash_history_path"; then
    printf '%s\n' 'cide: cannot select this panel history file (HISTFILE is readonly)' >&2
    # Exiting with the cleared list must not overwrite the readonly global file from a profile.
    set +o history
    exit 1
fi
if [ "${BASH_VERSINFO[0]}" -lt 4 ]; then
    shopt -u histappend
else
    shopt -s histappend
fi

_cide_bash_history_select() {
    local previous=$?
    HISTFILE=$_cide_bash_history_path
    return "$previous"
}
_cide_bash_history_flush() {
    local previous=$?
    if [ "${BASH_VERSINFO[0]}" -lt 4 ]; then
        # Bash 3.2's history -a skips saving when every in-memory entry is from this session
        # (its counter check uses < instead of <=). A new empty panel hits that every time.
        # Rewrite this panel's bounded list instead, including on exit. Reading no entries
        # from /dev/null updates Bash's file cursor so a user's history -n hook does not import
        # the snapshot we just wrote and duplicate the list before the next command.
        shopt -u histappend
        builtin history -w "$_cide_bash_history_path"
        builtin history -n /dev/null
    else
        builtin history -a "$_cide_bash_history_path"
    fi
    return "$previous"
}

# Indexed PROMPT_COMMAND arrays are executed in order by Bash 5.1+. Older Bash executes only
# element zero; keep its scalar behavior. Newlines also protect against a trailing comment in
# an existing prompt command. Both helpers return their incoming status for status prompts.
if [ "${BASH_VERSINFO[0]}" -gt 5 ] || {
    [ "${BASH_VERSINFO[0]}" -eq 5 ] && [ "${BASH_VERSINFO[1]}" -ge 1 ];
}; then
    case $(declare -p PROMPT_COMMAND 2>/dev/null) in
        'declare -a'*)
            PROMPT_COMMAND=(_cide_bash_history_select "${PROMPT_COMMAND[@]}" _cide_bash_history_flush)
            ;;
        *)
            PROMPT_COMMAND='_cide_bash_history_select
'${PROMPT_COMMAND-}'
_cide_bash_history_flush'
            ;;
    esac
else
    PROMPT_COMMAND='_cide_bash_history_select
'${PROMPT_COMMAND-}'
_cide_bash_history_flush'
fi
# Nested shells must not inherit Cide hook calls without their unexported helper functions.
export -n HISTFILE PROMPT_COMMAND
