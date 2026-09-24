/**
 * Proxy configuration: the environment cide gives the processes it starts.
 *
 * Every process, now, which is a change worth naming at the top of the file. This screen used
 * to close with a `Note` headed "Panes only" explaining that cide's own `git push` ignored it
 * and followed the login profile instead. That note is gone because the gap is gone:
 * `ProxyScope` answers separately for `claude`, for shell panes and for cide's own Git, and
 * the headless one-shot lane — which was never proxied at all — travels with `claude`.
 *
 * What replaced it is a harder sentence, kept on screen because it is the one a user cannot
 * discover by experiment: **not adding a proxy is not the same as removing one.** cide
 * inherits its own environment, so a child cide "leaves alone" is on whatever proxy cide was
 * launched with.
 *
 * Presentational like the rest of `sections.tsx`'s parts — it takes the stored value and a
 * `patch` callback and reads nothing else — so the whole screen can still be rendered from a
 * fixture.
 *
 * # Three modes, not a switch — and then three targets, for the same reason twice
 *
 * "Do not proxy" and "do not interfere" are different answers and a corporate laptop needs
 * both, so the mode is a three-way choice rather than an on/off. See `ProxyMode` in
 * `crates/cide-ipc/src/settings.rs` for the same argument from the data's side.
 *
 * The scope rows repeat the argument one axis over: *apply*, *leave alone* and *no proxy* are
 * three answers to "what does cide do to this child", and the middle one is the default for
 * Git because it is the only one that reproduces what every shipped version has done.
 *
 * # The readout is the honest part
 *
 * A proxy setting that does not show you the resulting environment is a setting you can only
 * debug by spawning a shell and typing `env | grep -i proxy`, which is exactly what a user
 * does when a pane cannot reach the network. So the exact variable names and values are
 * printed, in both spellings, including the ones being *removed*.
 *
 * There are three readouts now rather than one, because with a scope in play there is no such
 * thing as "what a child gets" — the same settings give `claude` a proxy and cide's own Git
 * nothing at all.
 *
 * The arithmetic behind those readouts is a mirror of `cide_core::proxy::ProxyEnv::resolve`,
 * and it lives in `./proxyEnv` rather than here so it can be executed: that module imports
 * nothing, and `ui/scripts/check-proxy.mjs` compiles it standalone, runs it against the cases
 * the Rust tests assert, and reads the constant lists and the target names out of the Rust to
 * prove they still match. Rust remains the authority — a divergence shows a wrong label and
 * never gives a child a wrong environment — but the label is the whole reason this screen
 * exists, so it is checked rather than asserted.
 *
 * `readoutKind` is in that module too, and deliberately: "nothing is set" has two meanings
 * here — deferring, and out of scope — and a two-branch sentence that reads as obvious is
 * exactly the kind that ships inverted. A rule inside a component is a rule no check script
 * can run.
 */
import { TextInput } from '@/kit/components/Field'
import { useState } from 'react'
import type { ProxyMode, ProxyScope, ProxySettings, ProxyTarget, SettingsPatch } from '@/ipc/client'
import { Group, Note, Row, Segmented } from './controls'
import {
  childEnvironment,
  hasUserinfo,
  claudeOnlyNote,
  isClaudeOnly,
  LOOPBACK,
  normalizeProxyUrl,
  readoutKind,
  redactProxyUrl,
  TARGETS,
  type EnvLine,
  type ProxyTargetName,
} from './proxyEnv'
import styles from './ProxySection.module.css'

export interface ProxySectionProps {
  proxy: ProxySettings
  patch: (patch: SettingsPatch) => void
}

const MODES: readonly { value: ProxyMode; label: string }[] = [
  { value: 'inherit', label: 'Inherit' },
  { value: 'manual', label: 'Use a proxy' },
  { value: 'direct', label: 'No proxy' },
]

/**
 * The three answers for one target. `Untouched` sits in the middle because it is the
 * do-nothing one, not because it is a compromise.
 *
 * "Leave alone" rather than "Untouched" as the visible word: the wire name is about cide's
 * behaviour and the label has to be about the child's, and every user reading this row is
 * asking "does cide interfere with this or not".
 */
const TARGET_OPTIONS: readonly { value: ProxyTarget; label: string }[] = [
  { value: 'configured', label: 'Apply' },
  { value: 'untouched', label: 'Leave alone' },
  { value: 'direct', label: 'No proxy' },
]

/**
 * One sentence per target per state — nine of them, and none of them shared.
 *
 * They are not interchangeable and the sharing is the trap. `Leave alone` on a shell pane
 * means "your shell profile decides"; on cide's own Git it means "whatever environment cide
 * itself was launched with, which may well be a proxy". A single generic sentence would be
 * true of neither.
 */
const TARGET_HINT: Record<keyof ProxyScope, Record<ProxyTargetName, string>> = {
  // Every child that talks to a model provider: the console whichever CLI Settings → Harness
  // names, every agent run whatever its role's harness, and the one-shot lane. One field for all
  // of them because to a user they are one thing — "my AI tools" — and a corporate proxy that
  // reached the claude console but not a codex run would be a failure nobody could place.
  claude: {
    configured:
      'Consoles (Claude Code or Codex), every agent run whatever its harness, and the one-shots behind Generate commit message and Explain selection all get the proxy above.',
    untouched:
      'cide adds and removes nothing for consoles and agent runs. They inherit whatever cide itself was launched with — which is a proxy, if your login profile exports one.',
    direct:
      'Every proxy variable is removed from consoles and agent runs, whatever the mode above says and whatever cide inherited.',
  },
  shells: {
    configured:
      'Your shell panes get the proxy above — so a curl, an npm install or a git pull typed into one follows this screen.',
    untouched:
      'Shell panes get cide’s own environment, unaltered. This is the closest thing to “not an IDE feature”: your shell behaves as your shell.',
    direct: 'Every proxy variable is removed from shell panes, whatever your profile exports.',
  },
  git: {
    configured:
      'The Push and Fetch/Pull buttons run git through the proxy above. This is a change from the default: cide has never put a proxy into its own git before.',
    untouched:
      'The default, and what every version of cide has done. git inherits cide’s environment — so if cide was started from a shell that exports a proxy, git uses it, and this screen is not what decided that.',
    direct:
      'The Push and Fetch/Pull buttons run git with every proxy variable removed. The one setting that can take cide’s git off a proxy your login profile exported.',
  },
}

export function ProxySection({ proxy, patch }: ProxySectionProps) {
  const set = (next: Partial<ProxySettings>) => patch({ proxy: { ...proxy, ...next } })
  const manual = proxy.mode === 'manual'
  const credentialed = manual && [proxy.http, proxy.https, proxy.all].some(hasUserinfo)
  const empty =
    manual && proxy.http.trim() === '' && proxy.https.trim() === '' && proxy.all.trim() === ''
  // What the HTTPS field will actually fall back to, shown as its placeholder: normalised,
  // because that is the string the child gets rather than the one that was typed, and
  // redacted, because a password entered in the field *above* must not be reprinted greyed
  // out in a field the user never put it in — that is the same screenshot the readout is
  // careful about, one control higher.
  const httpsFallback = normalizeProxyUrl(proxy.http)
  // Confirmation, never a preset button: a control that sets the three rows below it is a
  // fourth source of truth that can disagree with them the moment one is touched by hand.
  const claudeOnly = isClaudeOnly(proxy)

  return (
    <>
      <Group title="Proxy">
        <div className={styles.modeRow}>
          <Segmented
            label="Proxy mode"
            value={proxy.mode}
            options={MODES}
            onChange={(mode) => set({ mode })}
          />
          <p className={styles.modeHint}>{MODE_HINT[proxy.mode]}</p>
        </div>
      </Group>

      {manual && (
        <Group title="Addresses">
          <Field
            label="HTTP proxy"
            hint="HTTP_PROXY. A bare host:port is accepted — http:// is added, because Node throws on a URL without a scheme where curl would not."
            value={proxy.http}
            placeholder="http://proxy.example.com:3128"
            onCommit={(http) => set({ http })}
          />
          <Field
            label="HTTPS proxy"
            hint="HTTPS_PROXY. Leave empty to use the HTTP proxy above — one CONNECT proxy for both is the usual shape, and typing it twice is how one of the two goes stale."
            value={proxy.https}
            placeholder={
              httpsFallback === null
                ? 'http://proxy.example.com:3128'
                : redactProxyUrl(httpsFallback)
            }
            onCommit={(https) => set({ https })}
          />
          <Field
            label="ALL_PROXY"
            hint="Usually a SOCKS URL. No fallback from the HTTP proxy: this one covers protocols beyond the web, and pointing them somewhere you did not ask for is not a default worth having."
            value={proxy.all}
            placeholder="socks5://127.0.0.1:1080"
            onCommit={(all) => set({ all })}
          />
          <Field
            label="Bypass"
            hint={`NO_PROXY, comma separated. ${LOOPBACK.join(', ')} are always included.`}
            value={proxy.noProxy}
            placeholder="corp.internal, .example.com"
            onCommit={(noProxy) => set({ noProxy })}
          />
        </Group>
      )}

      {empty && (
        <Note title="Nothing is configured" tone="warn">
          This mode replaces the child's proxy variables with what is above, and above is
          empty — so children run with no proxy and any <code>HTTP_PROXY</code> from your shell
          profile is removed. That is the same result as <em>No proxy</em>, arrived at by
          accident.
        </Note>
      )}

      {credentialed && (
        <Note title="That URL contains a password" tone="warn">
          It is stored as typed in <code>workspace.json</code>, in plain text — a proxy that
          needs credentials cannot be used any other way until cide has a keyring. It is
          redacted everywhere it could otherwise be read back: the log, this screen's readout,
          and any <code>Debug</code> print of the settings. It is not redacted in the field
          above, because you have to be able to correct it.
        </Note>
      )}

      <Group title="Who gets it">
        {TARGETS.map(({ key, label }) => (
          <Row
            key={key}
            label={label}
            hint={TARGET_HINT[key][proxy.scope[key]]}
            control={
              <Segmented
                label={`Proxy for ${label}`}
                value={proxy.scope[key]}
                options={TARGET_OPTIONS}
                onChange={(target) => set({ scope: { ...proxy.scope, [key]: target } })}
              />
            }
          />
        ))}
      </Group>

      {/* The distinction the whole row above turns on, and the one a user cannot discover by
          experiment: not-adding a proxy and removing one are the same thing only on a machine
          that had no proxy to begin with. */}
      <Note title="“Leave alone” is not “No proxy”">
        cide inherits the environment it was launched from. If you started cide from a shell
        that exports <code>HTTPS_PROXY</code> — a login profile on a corporate laptop, say —
        then a child cide leaves alone <em>is already on that proxy</em>, and nothing on this
        screen put it there. <em>No proxy</em> is the only setting that removes it.
        <br />
        Even then: <code>git</code> still honours <code>http.proxy</code> from your
        <code>~/.gitconfig</code>, and cide does not edit your gitconfig. What cide can promise
        is the child’s environment, not everywhere the child looks.
      </Note>

      {claudeOnly && (
        <Note title="Consoles and agents only">
          The proxy above reaches the consoles, the agent runs and the one-shots — Claude Code
          and Codex alike — and nothing else cide starts.{' '}
          {claudeOnlyNote(proxy)}
        </Note>
      )}

      <Group title="What each child gets">
        {TARGETS.map(({ key, label }) => (
          <TargetReadout key={key} label={label} proxy={proxy} target={proxy.scope[key]} />
        ))}
      </Group>

      <Note title="Applied when a pane's process starts">
        A running pane keeps the environment it was spawned with — a process cannot be moved
        onto a different proxy from outside, and cide does not pretend otherwise. Close and
        reopen a pane, or relaunch cide, for a change here to reach it. Panes opened from now
        on already have it.
        <br />
        The Git rows are different, and better: Push and Fetch fork a fresh <code>git</code>
        every time, so a change there takes effect on the next click.
      </Note>
    </>
  )
}

/**
 * One target's readout, with the two kinds of emptiness told apart.
 *
 * The branch is `readoutKind`'s, not this component's: "nothing is set" has two meanings that
 * a user needs distinguished, and a rule that lives in a component is a rule no check script
 * can run. See `proxyEnv.ts`.
 */
function TargetReadout({
  label,
  proxy,
  target,
}: {
  label: string
  proxy: ProxySettings
  target: ProxyTarget
}) {
  const kind = readoutKind(proxy, target)
  return (
    <div className={styles.target}>
      <h4 className={styles.targetName}>{label}</h4>
      {kind === 'lines' ? (
        <Readout lines={childEnvironment(proxy, target)} />
      ) : kind === 'untouched' ? (
        <p className={styles.readoutEmpty}>
          Nothing at all. cide neither sets nor removes a proxy variable, so this child gets
          cide’s own environment exactly as it stands — <em>including</em> a proxy your login
          profile exported. Not even the loopback bypass is added.
        </p>
      ) : (
        <p className={styles.readoutEmpty}>
          Nothing of cide’s own. Whatever your shell profile exports reaches the child
          unchanged — plus <code>NO_PROXY={LOOPBACK.join(',')}</code>, but only if a proxy was
          inherited in the first place.
        </p>
      )}
    </div>
  )
}

const MODE_HINT: Record<ProxyMode, string> = {
  inherit:
    'Children get whatever your shell profile already exports. cide adds nothing and removes nothing — except that if something is inherited, the loopback bypass is merged in, so the IDE integration keeps working.',
  manual:
    "These addresses win over anything your profile exports, and a field left empty removes that variable rather than deferring to it. A setting that says “this is the proxy” and then quietly loses to a .bashrc is worse than no setting at all.",
  direct:
    'Every proxy variable, in both spellings, is scrubbed from the child. For the machine whose profile exports a proxy you do not want cide’s panes behind.',
}

function Readout({ lines }: { lines: EnvLine[] }) {
  if (lines.length === 0) {
    return (
      <p className={styles.readoutEmpty}>
        Nothing of cide’s own. Whatever your shell profile exports reaches the child unchanged
        — plus <code>NO_PROXY={LOOPBACK.join(',')}</code>, but only if a proxy was inherited in
        the first place.
      </p>
    )
  }
  return (
    <>
      <dl className={styles.readout}>
        {lines.map(({ name, value }) => (
          <div key={name} className={styles.readoutRow}>
            <dt className={styles.readoutName}>{name}</dt>
            <dd className={value === null ? styles.readoutUnset : styles.readoutValue}>
              {value === null ? 'removed from the child' : redactProxyUrl(value)}
            </dd>
          </div>
        ))}
      </dl>
      <p className={styles.readoutFoot}>
        Each name is also written in lower case — <code>http_proxy</code> and the rest — to the
        same value. curl reads only the lower-case spelling of <code>http_proxy</code>, while
        Go and much of the Node ecosystem prefer the upper-case one, and a pane that sets one
        of the two proxies half the commands typed into it.
      </p>
    </>
  )
}

interface FieldProps {
  label: string
  hint: string
  value: string
  placeholder: string
  onCommit: (next: string) => void
}

/**
 * A full-width text field, stacked rather than in a `Row`.
 *
 * `Row` puts its control hard right at its natural width, which is the correct shape for a
 * toggle and the wrong one for a URL long enough to need scrolling.
 *
 * Committed on blur and on Enter, never per keystroke, for the same reason `NumberField`
 * does it: a keystroke here is a workspace write, a `workspace.json` save and a broadcast to
 * every open window. For this field it is also a partially-typed password making the trip.
 * Escape abandons the draft and re-renders what is stored.
 */
function Field({ label, hint, value, placeholder, onCommit }: FieldProps) {
  const [draft, setDraft] = useState<string | null>(null)

  const commit = (text: string) => {
    setDraft(null)
    const next = text.trim()
    // Guarded, so tabbing through an untouched field is not a write and a broadcast.
    if (next !== value) onCommit(next)
  }

  return (
    <label className={styles.field}>
      <span className={styles.fieldLabel}>{label}</span>
      {/* A URL is a machine string: mono, like every path and counter in this app. */}
      <TextInput
        mono
        type="text"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        placeholder={placeholder}
        value={draft ?? value}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit(e.currentTarget.value)
          else if (e.key === 'Escape') setDraft(null)
        }}
      />
      <span className={styles.fieldHint}>{hint}</span>
    </label>
  )
}
