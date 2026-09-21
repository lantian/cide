/**
 * Whether a phone may reach this cide, and which devices have. (M72)
 *
 * This is the only screen in Settings that grants a capability to *another machine*. Everything
 * else here changes how cide looks or behaves for the person sitting at it; a paired device can
 * read the sessions on this computer, type into them, answer Claude's permission prompts, stop
 * and dispatch runs and edit the task board. So the switch is worded as the grant it is, the
 * grant is spelled out in a sentence on the screen where it is given, and the default is off.
 *
 * # Off, loopback, derive the port — and why each is separate
 *
 * Turning the listener **on** does not by itself put anything on a network: the bind is still
 * loopback until somebody chooses otherwise. Choosing *this network* is the second decision, and
 * choosing a **public** address is a third, behind its own toggle — "the people in my house" and
 * "everyone" are not the same answer and must not ride one switch.
 *
 * The port is derived from the profile unless the user types one, which is what makes a real
 * instance and a `[DEV]` instance both work with nothing configured. A number somebody typed is
 * a promise to a device that saved it, so a taken port is **refused by name** rather than slid
 * past — `cide_core::remote::port_for_profile` carries that argument.
 *
 * # There is no QR code yet, and the fallback is the point
 *
 * A code that has to be read off a screen and typed into a phone is eight characters of an
 * alphabet chosen so `I`, `L`, `O` and `U` cannot be misread, shown as `XXXX-XXXX` because eight
 * unbroken characters are read wrongly. That is the documented fallback for the QR, it needs no
 * dependency on either side, and it is what the pairing window shows today.
 *
 * # Nothing on this screen is a secret
 *
 * A device's token is shown once, to the device, at the moment it pairs. It is stored as a hash
 * in a `0600` file that this screen never reads, and `RemoteDevice` — the type the list is drawn
 * from — has no field that could carry one. That is `LlmProvider`'s rule reached from the other
 * end: rather than masking a credential in a group the screen round-trips, the credential is not
 * in the group.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  PairingAttempt,
  PairingInvite,
  PairingProgress,
  QrMatrix,
  RemoteBind,
  RemoteDevice,
  RemoteSettings,
  RemoteStatus,
  SettingsPatch,
} from '@/ipc/client'
import { remote } from '@/ipc/client'
import { events } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { ActionButton, Group, Note, NumberField, Row, Segmented, ToggleRow } from './controls'
import styles from './RemoteSection.module.css'

/** What a paired device is allowed to do. Stated where the permission is granted. */
const GRANT =
  'A paired device can see this cide’s projects, read and type into its sessions, answer permission prompts, stop and dispatch runs, and edit tasks.'

type BindChoice = 'loopback' | 'network'

export function RemoteSection({
  settings,
  patch,
}: {
  settings: RemoteSettings
  patch: (patch: SettingsPatch) => void
}) {
  const [status, setStatus] = useState<RemoteStatus>({ state: 'off' })
  const [devices, setDevices] = useState<readonly RemoteDevice[]>([])
  const [invite, setInvite] = useState<PairingInvite | null>(null)
  const [attempt, setAttempt] = useState<PairingAttempt | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  // A pairing window we have watched open. Closing the modal is driven by `open` going false,
  // and this is what stops an answer that was already in flight when the code was minted from
  // closing it before it was ever seen: only a window observed open may be observed to close.
  const opened = useRef(false)

  const refresh = useCallback(() => {
    void remote.status().then(setStatus)
    void remote.devices().then(setDevices)
    // Read on the same event as the rest, because a device reaching the pairing step raises it —
    // and so does the redemption that ends the window, which is what auto-closes the modal.
    void remote.pairingProgress().then((progress: PairingProgress) => {
      setAttempt(progress.attempt ?? null)
      if (progress.open) {
        opened.current = true
        return
      }
      if (opened.current) {
        opened.current = false
        setInvite(null)
      }
    })
  }, [])

  useEffect(() => {
    refresh()
    // The event carries no payload and this is why: the live half of the readout — how many
    // devices are connected right now — is read from the socket rather than stored, so a pushed
    // snapshot would be stale the instant it landed.
    const handle = events.onRemoteChanged(refresh)
    return () => {
      void handle.then((off) => off())
    }
  }, [refresh])

  const bind: BindChoice = settings.bind.kind === 'network' ? 'network' : 'loopback'

  const setBind = (next: BindChoice) => {
    const kind: RemoteBind = next === 'network' ? { kind: 'network' } : { kind: 'loopback' }
    patch({ remote: { ...settings, bind: kind } })
  }

  const startPairing = () => {
    setFailure(null)
    opened.current = false
    void remote
      .pairingStart()
      .then(setInvite)
      .catch((error: unknown) => setFailure(errorText(error)))
  }

  const cancelPairing = () => {
    opened.current = false
    setInvite(null)
    setAttempt(null)
    void remote.pairingCancel()
  }

  return (
    <>
      <Group title="Remote access">
        <ToggleRow
          label="Let a phone connect to this cide"
          hint={GRANT}
          checked={settings.enabled}
          onChange={(enabled) => patch({ remote: { ...settings, enabled } })}
        />
        <Row
          label="Reachable from"
          hint="Loopback means this machine only, which is no use to a phone. This network binds every interface, which is what a laptop that moves between networks needs."
          control={
            <Segmented
              value={bind}
              options={[
                { value: 'loopback', label: 'This machine' },
                { value: 'network', label: 'This network' },
              ]}
              onChange={setBind}
              label="Reachable from"
            />
          }
        />
        <Row
          label="Port"
          hint="Zero derives one from this profile, so a real instance and a [DEV] instance differ without being configured. A port you choose is refused if it is taken, rather than moved."
          control={
            <NumberField
              value={settings.port}
              min={0}
              max={65535}
              step={1}
              onChange={(port) => patch({ remote: { ...settings, port } })}
              label="Port"
            />
          }
        />
        <ToggleRow
          label="Allow a public address"
          hint="Off, cide refuses to bind an address that is routable from the internet. Reaching this machine from outside your network is a different decision from reaching it from inside one."
          checked={settings.allowNonPrivate}
          onChange={(allowNonPrivate) => patch({ remote: { ...settings, allowNonPrivate } })}
        />
      </Group>

      <Group title="Status">
        <StatusReadout status={status} />
      </Group>

      <Group title="Paired devices">
        {devices.length === 0 ? (
          <Note>No device is paired. Nothing is listening until one is, or until a pairing code is open.</Note>
        ) : (
          <ul className={styles.devices}>
            {devices.map((device) => (
              <DeviceRow key={device.id} device={device} onForget={refresh} />
            ))}
          </ul>
        )}
        {invite ? (
          <Invite invite={invite} attempt={attempt} onCancel={cancelPairing} />
        ) : (
          <ActionButton
            label="Pair a device…"
            onClick={startPairing}
            disabled={!settings.enabled}
          />
        )}
        {failure ? <Note tone="warn">{failure}</Note> : null}
      </Group>
    </>
  )
}

function StatusReadout({ status }: { status: RemoteStatus }) {
  if (status.state === 'off') {
    return <Note>Not listening.</Note>
  }
  if (status.state === 'refused') {
    // Named rather than shown as an empty address list, which reads as "starting…".
    return <Note tone="warn">{status.why}</Note>
  }
  return (
    <div className={styles.status}>
      <p className={styles.line}>
        {status.connected === 0
          ? 'Listening. No device is connected.'
          : `Listening. ${status.connected} device${status.connected === 1 ? '' : 's'} connected.`}
      </p>
      <ul className={styles.addresses}>
        {status.addresses.map((address) => (
          <li key={address} className={styles.address}>
            {address.includes(':') ? `[${address}]` : address}:{status.port}
          </li>
        ))}
      </ul>
    </div>
  )
}

function DeviceRow({ device, onForget }: { device: RemoteDevice; onForget: () => void }) {
  return (
    <li className={styles.device}>
      <span className={styles.name}>{device.name || 'Unnamed device'}</span>
      <span className={styles.meta}>
        {device.platform || 'unknown'} ·{' '}
        {device.lastSeenUnixMs === 0
          ? 'never connected'
          : `last seen ${new Date(device.lastSeenUnixMs).toLocaleString()}`}
        {device.lastAddr ? ` · ${device.lastAddr}` : ''}
      </span>
      <ActionButton
        label="Revoke"
        onClick={() => {
          void remote.deviceForget(device.id).then(onForget)
        }}
      />
    </li>
  )
}

/// The pairing QR, drawn as one `<path>`.
///
/// One path and not a rect per module: a version-7 symbol is around 2 000 dark modules, and
/// 2 000 elements in the DOM of a settings dialog is a measurable cost for a picture that never
/// changes. The `d` is a run of `M x y h w v1 h-w z` subpaths, which `fill-rule` renders
/// identically and the renderer builds in one pass.
///
/// `shapeRendering="crispEdges"` is load-bearing rather than cosmetic: antialiasing a
/// module boundary greys the edge between a dark and a light module, and a scanner thresholding
/// a photograph of that reads the grey as whichever side it lands on. It is the difference
/// between a code that scans in a lit room and one that scans only sometimes.
function QrCode({ matrix }: { matrix: QrMatrix }) {
  // A quiet zone is part of the symbol, not a margin: the specification asks for four modules
  // of light on every side, and a scanner that cannot find one may refuse a code that is
  // otherwise perfect. Drawing it into the viewBox means no surrounding layout can eat it.
  const quiet = 4
  const span = matrix.size + quiet * 2

  // Horizontal runs are merged as they are walked. It costs one comparison per module and
  // roughly halves the path on a symbol of this density.
  let d = ''
  for (let y = 0; y < matrix.size; y++) {
    let x = 0
    while (x < matrix.size) {
      if (!matrix.modules[y * matrix.size + x]) {
        x++
        continue
      }
      let run = 1
      while (x + run < matrix.size && matrix.modules[y * matrix.size + x + run]) run++
      d += `M${x + quiet} ${y + quiet}h${run}v1h-${run}z`
      x += run
    }
  }

  return (
    <svg
      className={styles.qr}
      viewBox={`0 0 ${span} ${span}`}
      role="img"
      aria-label="Pairing code as a QR code"
      shapeRendering="crispEdges"
    >
      {/* The light modules are this rect, not the panel behind it. A QR on a dark theme must
          still be dark-on-light: a scanner can invert, but not every one does, and the cost of
          being wrong is a code that silently does not read. */}
      <rect width={span} height={span} fill="#ffffff" />
      <path d={d} fill="#000000" />
    </svg>
  )
}

/// The six digits, and the sentence that makes them mean something.
///
/// This is the typed road's only defence against somebody relaying the connection, and it works
/// exactly as well as the person's willingness to actually look. So the number is the largest
/// thing on the panel, the address it belongs to is beside it — "a device is pairing" is worth
/// being able to disbelieve, and an address that is not your phone's is the whole warning — and
/// the instruction is phrased as a thing to do rather than a thing to know.
///
/// There is no button here, and that is the design rather than an omission. cide cannot tell
/// whether the digits matched; only the person can, and the tap that matters is on the phone,
/// which is what actually withholds the code. A **Confirm** on this side would be a button that
/// changes nothing, and a habit of pressing it is precisely the habit that defeats the check.
function Compare({ attempt }: { attempt: PairingAttempt }) {
  return (
    <div className={styles.compare}>
      <p className={styles.line}>
        A device at <span className={styles.addr}>{attempt.addr}</span> is pairing. It should be
        showing these six digits — if it shows anything else, tap <em>No</em> on the device.
      </p>
      <p className={styles.sas}>{attempt.sas}</p>
    </div>
  )
}

function Invite({
  invite,
  attempt,
  onCancel,
}: {
  invite: PairingInvite
  attempt: PairingAttempt | null
  onCancel: () => void
}) {
  return (
    <div className={styles.invite}>
      {attempt !== null ? <Compare attempt={attempt} /> : null}
      {invite.qr ? (
        <>
          <p className={styles.line}>Scan this with the app:</p>
          <QrCode matrix={invite.qr} />
          <p className={styles.line}>…or type it in, with the address above:</p>
        </>
      ) : (
        <p className={styles.line}>Type this into the app, with the address above:</p>
      )}
      <p className={styles.code}>{invite.grouped}</p>
      <Note>
        Good for two minutes, and for one device. A wrong code cancels it — type it carefully
        rather than quickly.
      </Note>
      <ActionButton label="Cancel" onClick={onCancel} />
    </div>
  )
}
