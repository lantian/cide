/**
 * Proxy configuration: the environment cide gives the processes it starts in panes.
 *
 * Panes, not "every child" — `cide_git::push` runs its own `git` and does not read this. The
 * closing `Note` says so on screen, because that gap is silent and expensive: a user fixes
 * their shells here and then watches the Git tool window hang with nothing connecting the two.
 *
 * Presentational like the rest of `sections.tsx`'s parts — it takes the stored value and a
 * `patch` callback and reads nothing else — so the whole screen can still be rendered from a
 * fixture.
 *
 * # Three modes, not a switch
 *
 * "Do not proxy" and "do not interfere" are different answers and a corporate laptop needs
 * both, so the control is a three-way choice rather than an on/off. See `ProxyMode` in
 * `crates/cide-ipc/src/settings.rs` for the same argument from the data's side.
 *
 * # The readout is the honest part
 *
 * A proxy setting that does not show you the resulting environment is a setting you can only
 * debug by spawning a shell and typing `env | grep -i proxy`, which is exactly what a user
 * does when a pane cannot reach the network. So the exact variable names and values are
 * printed, in both spellings, including the ones being *removed*.
 *
 * The arithmetic behind that readout is a mirror of `proxy_env` in
 * `crates/cide-app/src/cmd/session.rs`, and it lives in `./proxyEnv` rather than here so it
 * can be executed: that module imports nothing, and `ui/scripts/check-proxy.mjs` compiles it
 * standalone, runs it against the cases the Rust tests assert, and reads the two constant
 * lists out of `session.rs` to prove they still match. Rust remains the authority — a
 * divergence shows a wrong label and never gives a child a wrong environment — but the label
 * is the whole reason this screen exists, so it is checked rather than asserted.
 */
import { useState } from 'react'
import type { ProxyMode, ProxySettings, SettingsPatch } from '@/ipc/client'
import { Group, Note, Segmented } from './controls'
import {
  childEnvironment,
  hasUserinfo,
  LOOPBACK,
  normalizeProxyUrl,
  redactProxyUrl,
  type EnvLine,
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

      <Group title="What a child gets">
        <Readout lines={childEnvironment(proxy)} />
      </Group>

      <Note title="Applied when a pane's process starts">
        A running pane keeps the environment it was spawned with — a process cannot be moved
        onto a different proxy from outside, and cide does not pretend otherwise. Close and
        reopen a pane, or relaunch cide, for a change here to reach it. Panes opened from now
        on already have it.
      </Note>

      {/* Said out loud because the surprise is expensive and silent: a corporate user fixes
          their panes here, watches the Git tool window hang on push, and has no reason at all
          to connect the two. See the note on `proxy_env` in `cmd/session.rs`. */}
      <Note title="Panes only">
        This is the environment cide gives the processes it starts <em>in panes</em> — your
        shells and <code>claude</code>. cide’s own Git operations are not panes: a push or
        fetch from the Git tool window runs <code>git</code> with the environment cide itself
        was launched with, so it follows your login profile, not this screen. Configure that
        one in <code>~/.gitconfig</code> as <code>http.proxy</code>.
      </Note>
    </>
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
      <input
        className={styles.input}
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
