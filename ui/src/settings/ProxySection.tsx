/**
 * Proxy configuration: what every child cide spawns gets in its environment.
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
 * [`childEnvironment`] is therefore a mirror of `proxy_env` in
 * `crates/cide-app/src/cmd/session.rs`. Rust is the authority; a divergence here shows the
 * user a wrong label, never gives a child a wrong environment. It is small and both sides are
 * commented as a pair for that reason.
 */
import { useState } from 'react'
import type { ProxyMode, ProxySettings, SettingsPatch } from '@/ipc/client'
import { Group, Note, Segmented } from './controls'
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
 * The loopback hosts cide adds to `NO_PROXY` and will not let you remove.
 *
 * Kept in step with `LOOPBACK_EXEMPT` in `cmd/session.rs`. The IDE integration is an MCP
 * server on loopback; a proxy that swallows it takes inline diffs and @-mentions with it,
 * silently.
 */
const LOOPBACK = ['localhost', '127.0.0.1', '::1'] as const

export function ProxySection({ proxy, patch }: ProxySectionProps) {
  const set = (next: Partial<ProxySettings>) => patch({ proxy: { ...proxy, ...next } })
  const manual = proxy.mode === 'manual'
  const credentialed = manual && [proxy.http, proxy.https, proxy.all].some(hasUserinfo)
  const empty =
    manual && proxy.http.trim() === '' && proxy.https.trim() === '' && proxy.all.trim() === ''

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
            placeholder={proxy.http.trim() === '' ? 'http://proxy.example.com:3128' : proxy.http}
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

/** One environment line in the readout, or one being removed. */
interface EnvLine {
  name: string
  /** `null` when the variable is removed from the child rather than set. */
  value: string | null
}

/**
 * The variables a child would be spawned with, as a mirror of `proxy_env` in
 * `crates/cide-app/src/cmd/session.rs`.
 *
 * Only the upper-case name is listed, with the note below explaining that the lower-case
 * twin is set to the same value. Printing eight rows where four carry the information reads
 * as a bug in the readout rather than as the deliberate redundancy it is.
 *
 * The inherit case cannot be shown honestly from here — it depends on the environment *this
 * process* was launched with, which the webview cannot see — so it returns nothing and the
 * prose above says what happens instead.
 */
function childEnvironment(proxy: ProxySettings): EnvLine[] {
  if (proxy.mode === 'inherit') return []
  if (proxy.mode === 'direct') {
    return ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY'].map((name) => ({
      name,
      value: null,
    }))
  }
  const http = normalize(proxy.http)
  return [
    { name: 'HTTP_PROXY', value: http },
    { name: 'HTTPS_PROXY', value: normalize(proxy.https) ?? http },
    { name: 'ALL_PROXY', value: normalize(proxy.all) },
    { name: 'NO_PROXY', value: bypassList(proxy.noProxy) },
  ]
}

/** Mirrors `normalize_proxy_url`: blank is unset, and a bare host:port gains `http://`. */
function normalize(raw: string): string | null {
  const trimmed = raw.trim()
  if (trimmed === '') return null
  return trimmed.includes('://') ? trimmed : `http://${trimmed}`
}

/** Mirrors `no_proxy_value`: loopback first, then the user's entries, deduplicated. */
function bypassList(extra: string): string {
  const entries: string[] = [...LOOPBACK]
  for (const raw of extra.split(',')) {
    const entry = raw.trim()
    if (entry === '') continue
    if (!entries.some((e) => e.toLowerCase() === entry.toLowerCase())) entries.push(entry)
  }
  return entries.join(',')
}

/** Whether a URL carries userinfo, i.e. whether it can be carrying a password. */
function hasUserinfo(url: string): boolean {
  const rest = url.split('://')[1] ?? url
  const authority = rest.split('/')[0] ?? ''
  return authority.includes('@')
}

/**
 * Mirrors `redact_proxy_url`: the host survives, the credentials do not.
 *
 * Applied to the readout and not to the input field. The readout is the part of this screen
 * that ends up in a screenshot attached to a ticket; the field is the part you have to be
 * able to correct.
 */
function redact(url: string): string {
  const at = url.indexOf('://')
  if (at < 0) return url.includes('@') ? '***' : url
  const scheme = url.slice(0, at)
  const rest = url.slice(at + 3)
  const slash = rest.indexOf('/')
  const authority = slash < 0 ? rest : rest.slice(0, slash)
  const tail = slash < 0 ? '' : rest.slice(slash)
  const marker = authority.lastIndexOf('@')
  if (marker < 0) return url
  return `${scheme}://***@${authority.slice(marker + 1)}${tail}`
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
              {value === null ? 'removed from the child' : redact(value)}
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
