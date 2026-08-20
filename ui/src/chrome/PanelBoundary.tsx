/**
 * A wall around one panel, so that a bug inside it cannot take the window with it.
 *
 * # Why this exists
 *
 * React unmounts the **entire root** when a render throws and no boundary catches it. This app
 * had none — not one `componentDidCatch` anywhere — so any error in any panel emptied the whole
 * window: no chrome, no tabs, no terminals, no way to reach the control that would put it back.
 * A user hit exactly that opening the Git panel, and the sentence that matters is the second
 * half of their report: *"even after cide restart — stays empty."*
 *
 * That second half is the real defect, and it is structural rather than particular to whichever
 * panel threw. Which panel is showing is state, and state is restored on launch — so a panel
 * that throws on mount throws again on the next launch, and again after that. The failure is not
 * "a panel is broken", it is "the IDE is broken, permanently, with no message". Every gesture
 * that could recover it lives in the chrome that the throw just unmounted.
 *
 * A boundary turns that into: one panel says it failed, everything else keeps working, and the
 * fallback carries the button that closes it. **Recovery has to be reachable from inside the
 * failure** — that is the whole design, and it is why the fallback is a control and not a
 * message. Automatically writing the panel shut instead was rejected: a panel that closes itself
 * on the way past looks like the click did nothing, and a user who cannot see what happened
 * cannot report it either.
 *
 * # What it deliberately does not do
 *
 * It does not retry, and it does not reset itself when props change. A render that threw once
 * throws again on the same inputs, and a boundary that re-renders optimistically produces a
 * flicker loop that is harder to read than a stopped panel. The reset is the explicit gesture in
 * the fallback.
 *
 * It also does not catch anything outside render: an event handler, a `setTimeout`, or a
 * rejected promise never reaches a boundary. Those already fail without emptying the window,
 * which is why this is scoped to the one failure mode that does.
 */
import { Component, type ErrorInfo, type ReactNode } from 'react'

export interface PanelBoundaryProps {
  /** What to name in the fallback — "Git", "Files", "the git tool window". */
  readonly name: string
  /**
   * Close this panel. Rendered as the recovery button when present.
   *
   * Optional because not every panel has a close that makes sense, and a button that names an
   * action the caller cannot perform is worse than no button.
   */
  readonly onClose?: (() => void) | undefined
  readonly children: ReactNode
}

interface PanelBoundaryState {
  /** The error, or `null` while the subtree is healthy. */
  readonly error: Error | null
}

export class PanelBoundary extends Component<PanelBoundaryProps, PanelBoundaryState> {
  override state: PanelBoundaryState = { error: null }

  static getDerivedStateFromError(error: unknown): PanelBoundaryState {
    return { error: error instanceof Error ? error : new Error(String(error)) }
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    /*
     * To the console and nowhere else.
     *
     * `--inspect` routes the webview console into the Rust log, so this is what a bug report can
     * actually carry, and the component stack is the half that names which panel and which
     * element — a message alone rarely does. Not a toast: the fallback below is already on
     * screen saying the same thing, and two reports of one failure read as two failures.
     */
    console.error(`[cide] ${this.props.name} panel failed to render`, error, info.componentStack)
  }

  override render(): ReactNode {
    const { error } = this.state
    if (error === null) return this.props.children
    return (
      <div role="alert" data-audit="panelFailed" style={FALLBACK}>
        <strong>{this.props.name} could not be drawn.</strong>
        {/* The message verbatim. A panel that failed and will not say why leaves the user
            nothing to paste into a report, and this is the only place the text survives. */}
        <code style={MESSAGE}>{error.message}</code>
        <span style={NOTE}>
          The rest of the window is unaffected. Close this panel to carry on; it will come back
          the next time you open it.
        </span>
        {this.props.onClose !== undefined && (
          <button type="button" onClick={this.props.onClose} style={BUTTON}>
            Close {this.props.name}
          </button>
        )}
      </div>
    )
  }
}

/*
 * Inline styles, and that is the one place this file breaks the house rule on purpose: a CSS
 * module is another import that can be the thing that failed. A fallback with a broken
 * stylesheet is an unstyled pile of text at the moment the user most needs to read it.
 */
const FALLBACK: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: '8px',
  alignItems: 'flex-start',
  padding: '16px',
  overflow: 'auto',
  minWidth: 0,
  color: 'var(--fg, #ddd)',
  font: '12px/1.5 var(--font-ui, system-ui, sans-serif)',
}

const MESSAGE: React.CSSProperties = {
  font: '11px/1.4 var(--font-mono, ui-monospace, monospace)',
  color: 'var(--warn, #e5a50a)',
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-word',
  maxWidth: '100%',
}

const NOTE: React.CSSProperties = { opacity: 0.7 }

const BUTTON: React.CSSProperties = {
  padding: '4px 10px',
  cursor: 'pointer',
  color: 'inherit',
  background: 'var(--chrome-hi, #333)',
  border: '1px solid var(--border, #555)',
  borderRadius: '3px',
  font: 'inherit',
}
