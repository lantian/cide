/**
 * The propose/explore composer, as a function of its props. (M28)
 *
 * Its own file, and that is a layering rule rather than tidiness: `ProposeDialog.tsx` reaches the
 * workspace store, `@/ipc/client` and `revealPane`, and `check:openspec-render` SSR-bundles every
 * view it renders — so a pure view living beside that host drags **xterm** into a node process,
 * where the bundle dies on `self is not defined`. It did, once, on the first attempt at this
 * split. The views this directory checks import `model.ts` and a stylesheet and nothing else.
 *
 * # Why the box exists at all, rather than the button just sending
 *
 * Both commands take free text — *"the change name (kebab-case), OR a description of what the
 * user wants to build"* — and a bare one is legal and useless: Claude answers by asking what to
 * propose, which is a round trip we had the user's attention for and gave away.
 *
 * # The line is read, never spelled
 *
 * `line` arrives as a prop, off the project's board. OpenSpec moved `/opsx:propose` to
 * `/openspec-propose`, and every surface with the old spelling baked in previewed a line no
 * project had. This is also the one place a user can see that a description typed across three
 * lines arrives as one turn — the command is typed into a PTY and ended with Enter, so a newline
 * in the middle would submit the first half on its own.
 */
import styles from './OpenSpecPanel.module.css'
import { ASK_CANCEL, ASK_SEND, askPlaceholder, askTitle, canAsk, commandLine } from './model'

export interface ProposeFormProps {
  /** `propose` or `explore` — cide's handle, never the invocation. */
  command: string
  /** The line this project would actually type, off its board. See [`invocation`]. */
  line: string
  text: string
  busy: boolean
  onText: (text: string) => void
  onSend: () => void
  onCancel: () => void
}

/**
 * The form, as a function of its props.
 *
 * Split from the host below for the reason every other surface in this directory is: the render
 * check SSRs it with fixed fixtures, and `renderToStaticMarkup` runs no effects — so a form that
 * read a store would be gated on nothing the check could vary.
 */
export function ProposeFormView(props: ProposeFormProps) {
  const { command, line, text, busy } = props
  const ready = canAsk(command, text) && !busy
  return (
    <div className={styles.ask} data-audit="openspecAsk" data-command={command}>
      <p className={styles.askHeading}>{askTitle(command)}</p>
      <p className={styles.askTitle}>{line}</p>
      <textarea
        className={styles.askInput}
        data-audit="openspecAskInput"
        data-write="true"
        rows={5}
        autoFocus
        placeholder={askPlaceholder(command)}
        value={text}
        disabled={busy}
        onKeyDown={(event) => {
          // Enter sends, Shift+Enter is a newline. The line is flattened on the way out — by
          // `commandLine` for the preview and by Rust for the write — so a description typed
          // across three lines still arrives as one turn.
          if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            if (ready) props.onSend()
            return
          }
          if (event.key === 'Escape') {
            event.preventDefault()
            props.onCancel()
          }
        }}
        onChange={(event) => props.onText(event.target.value)}
      />
      <p className={styles.askPreview} data-audit="openspecAskPreview">
        {commandLine(line, text)}
      </p>
      <div className={styles.actions}>
        <button
          type="button"
          className={cx(styles.button, ready && styles.primary)}
          data-audit="openspecAskSend"
          data-write="true"
          disabled={!ready}
          title={
            ready
              ? 'Types this into the project’s Claude tab and takes you to it'
              : 'Say what the change should do first'
          }
          onClick={props.onSend}
        >
          {ASK_SEND}
        </button>
        <button
          type="button"
          className={styles.button}
          data-audit="openspecAskCancel"
          disabled={busy}
          onClick={props.onCancel}
        >
          {ASK_CANCEL}
        </button>
      </div>
    </div>
  )
}

function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

