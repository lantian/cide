# Sourced by the first PROMPT_COMMAND with automatic login profiles disabled. Apple's Bash
# bypasses the POSIX ENV entry. Run profiles ourselves before installing their prompt hooks.
# This script stays compatible with Bash 3.2.
set +o posix
# Replace the bootstrap with the inherited hook before profiles inspect or extend it.
if [ "$_CIDE_BASH_PROMPT_COMMAND_WAS_SET" = 1 ]; then
    PROMPT_COMMAND=$_CIDE_BASH_ORIGINAL_PROMPT_COMMAND
else
    unset PROMPT_COMMAND
fi
unset _CIDE_BASH_PROMPT_COMMAND_WAS_SET _CIDE_BASH_ORIGINAL_PROMPT_COMMAND
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

# Bash already loaded history before the first prompt. Profiles may reset HISTFILE or import
# shared history, so discard that list and reload only the selected pane file.
builtin history -c
if ! builtin export "HISTFILE=$_cide_bash_history_path"; then
    printf '%s\n' 'cide: cannot select this panel history file (HISTFILE is readonly)' >&2
    # Exiting with the cleared list must not overwrite the readonly global file from a profile.
    set +o history
    exit 1
fi
builtin history -r "$_cide_bash_history_path"
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

# The first prompt called the bootstrap instead of the user's hooks. Run the installed hooks
# now too, respecting the version's array behavior, so the first prompt is ready for input.
if [ "${BASH_VERSINFO[0]}" -gt 5 ] || {
    [ "${BASH_VERSINFO[0]}" -eq 5 ] && [ "${BASH_VERSINFO[1]}" -ge 1 ];
}; then
    for _cide_bash_prompt_command in "${PROMPT_COMMAND[@]}"; do
        builtin eval "$_cide_bash_prompt_command"
    done
    unset _cide_bash_prompt_command
else
    # The failed version test is not a command the user ran; start the first prompt at zero.
    :
    builtin eval "${PROMPT_COMMAND-}"
fi
