import { useState, type ReactElement } from 'react'

import { Button } from '../../components/Button'
import { Field, TextInput } from '../../components/Field'
import { Note } from '../../components/Feedback'
import {
  Dialog,
  Menu,
  Picker,
  PickerFoot,
  PickerFrame,
  PickerHint,
  PickerInput,
  PickerList,
  PickerMatch,
  PickerRow,
  PickerStatus,
  Scrim,
  Tooltip,
  Wizard,
} from '../../components/Overlay'
import { Kbd } from '../../components/Status'
import { Heading, PathList, PathRow } from '../../components/Surface'
import { Icon } from '@/icons/Icon'
import { Cell, Chapter, Line, Specimen } from '../Specimen'

export function Overlays(): ReactElement {
  const [step, setStep] = useState(1)
  const [open, setOpen] = useState(false)
  const steps = ['Type', 'Location', 'Roles', 'Create'] as const
  return (
    <Chapter
      id="overlays"
      title="Dialogs, menus, pickers"
      lead={
        <>
          Everything that floats. One dialog frame (the app has 10 header and 11 footer copies
          today), the wizard frame, the command-palette picker, one menu, and a tooltip — the app
          has none and uses the native <code>title</code> about 200 times. Shown inline here; on
          screen they sit on the <code>Scrim</code>.
        </>
      }
    >
      <Specimen
        name="Dialog"
        source="Overlay.tsx › Dialog, Scrim"
        use={
          <>
            Title and one lead sentence, the body, a footer with an optional note on the left and
            the buttons on the right, <strong>primary last</strong>. When the dialog confirms
            something destructive, the <em>safe</em> button is the primary and has the focus; the
            destructive one is <code>danger</code>.
          </>
        }
        ground="bg"
        spec={[
          ['frame', 'width narrow 420 · default 520 · picker 620 · wide 880; --chrome, 1px --border, --r-4, --shadow-3'],
          ['height', 'at most the window less 128; the body scrolls, head and footer stay'],
          ['flush', 'a body list edge to edge under a --border-soft rule'],
          ['head / body / foot padding', '16 · 28 horizontally (× --ui-scale)'],
          ['title / lead', '--fs-ui-16 600 --text-hi / --fs-ui-13 --dim'],
          ['footer', 'top 1px --border, note --fs-ui-11 --faint, gap --sp-4'],
        ]}
      >
        <Dialog
          title="Rename branch"
          lead="Renames it locally. The remote branch keeps its name until you push."
          onClose={() => {}}
          below="Renamed branches keep their upstream; push to rename the remote."
          footNote="feature/ui-kit → feature/design-kit"
          actions={
            <>
              <Button>Cancel</Button>
              <Button variant="primary">Rename</Button>
            </>
          }
        >
          <Field label="New name">
            {({ id }) => <TextInput id={id} mono defaultValue="feature/design-kit" />}
          </Field>
        </Dialog>
        <Dialog
          title="Discard 3 changed files?"
          titleAside="1 of 2"
          lead="The changes are not in any commit and cannot be brought back."
          width="narrow"
          flush
          actions={
            <>
              <Button variant="danger">Discard</Button>
              <Button variant="primary">Keep them</Button>
            </>
          }
        >
          <PathList>
            <PathRow mark={<Icon name="minus" size={1} />} name="Kit.tsx" where="ui/src/kit" />
            <PathRow mark={<Icon name="minus" size={1} />} name="Overlay.tsx" where="ui/src/kit/components" />
            <PathRow mark={<Icon name="minus" size={1} />} name="ui-kit.md" where="docs" />
          </PathList>
        </Dialog>
        <Cell label="on the scrim">
          <Button onClick={() => setOpen(true)}>Open a dialog</Button>
        </Cell>
        {open && (
          <Scrim onDismiss={() => setOpen(false)}>
            <Dialog
              title="Close the project?"
              lead="Running sessions keep running; the project reopens where you left it."
              onClose={() => setOpen(false)}
              actions={
                <>
                  <Button onClick={() => setOpen(false)}>Cancel</Button>
                  <Button variant="primary" onClick={() => setOpen(false)}>
                    Close project
                  </Button>
                </>
              }
            />
          </Scrim>
        )}
      </Specimen>

      <Specimen
        name="Wizard"
        source="Overlay.tsx › Wizard, Stepper"
        use="A task of three or more steps that each need a screen. The rail carries the brand, the steps (done with a check, current in the gradient, ahead numbered) and a note; the footer keeps one height on every step so Continue does not jump."
        ground="bg"
      >
        <Wizard
          brand="New project"
          brandIcon="square-plus"
          steps={steps}
          current={step}
          railNote="You can change all of this later in the project's settings."
          footNote={`Step ${step + 1} of ${steps.length}`}
          actions={
            <>
              <Button disabled={step === 0} onClick={() => setStep((s) => Math.max(0, s - 1))}>
                Back
              </Button>
              <Button
                variant="primary"
                trailingIcon="arrow-right"
                onClick={() => setStep((s) => Math.min(steps.length - 1, s + 1))}
              >
                Continue
              </Button>
            </>
          }
        >
          <Heading title={`${steps[step] ?? ''}`} lead="Every step is a heading, one lead sentence and its fields." />
          <Field label="Location" hint="The folder is created if it does not exist.">
            {({ id, describedBy }) => (
              <TextInput id={id} aria-describedby={describedBy} icon="folder" mono defaultValue="~/work/new-project" />
            )}
          </Field>
          <div style={{ marginTop: 'var(--sp-5)' }}>
            <Note tone="ok">The folder is empty and writable.</Note>
          </div>
        </Wizard>
      </Specimen>

      <Specimen
        name="Picker"
        source="Overlay.tsx › Picker"
        use="Type to find, arrows to move, Enter to go: the command palette, the file picker, go to symbol, switch branch. One frame for all of them; the matched characters are in the accent."
        ground="bg"
      >
        <Picker
          placeholder="Type a command"
          query="split"
          selected="split-right"
          rows={[
            {
              id: 'split-right',
              icon: 'columns-2',
              label: ['', 'Split', ' pane right'],
              detail: 'Layout',
              trailing: <Kbd keys={['Ctrl', '\\']} />,
            },
            {
              id: 'split-down',
              icon: 'rows-2',
              label: ['', 'Split', ' pane down'],
              detail: 'Layout',
            },
            {
              id: 'diff-split',
              icon: 'file-diff',
              label: ['Toggle ', 'split', ' diff'],
              detail: 'Git',
            },
          ]}
          foot={
            <>
              <PickerHint keys="↑↓">move</PickerHint>
              <PickerHint keys="⏎">run</PickerHint>
              <PickerHint keys="Esc">close</PickerHint>
            </>
          }
        />
      </Specimen>

      <Specimen
        name="Picker, in parts"
        source="Overlay.tsx › PickerFrame, PickerInput, PickerList, PickerRow, PickerMatch, PickerStatus, PickerFoot, PickerHint"
        use="What the app's pickers are built from, because their lists are virtualised and their field draws its own caret: the frame, the input row (lead, the caller's field, a counter), the scroll container, rows (selected, disabled, or placed by a virtualiser), a status line in the list's place, and the foot of hints. narrow is 340 wide, for a one-field popup."
        ground="bg"
        spec={[
          ['frame', '620 (narrow 340), --chrome, --r-4, --shadow-3, at most the window less 128'],
          ['input row', '44 tall; lead and counter mono; field --fs-ui-14 --text-hi'],
          ['row', '30 tall, --r-2, inset --sp-2 in the list; selected --sel, its icon --accent'],
          ['foot', 'hints --fs-ui-11 --faint, keys mono --dim'],
        ]}
      >
        <PickerFrame label="Go to file" narrow>
          <PickerInput lead=">" trailing="2 of 418">
            <input defaultValue="butt" aria-label="Go to file" />
          </PickerInput>
          <PickerList>
            <PickerRow selected>
              <Icon name="file-code" size={1} />
              <span>
                <PickerMatch>Butt</PickerMatch>on.tsx
              </span>
            </PickerRow>
            <PickerRow disabled>
              <Icon name="file-code" size={1} />
              <span>
                <PickerMatch>Butt</PickerMatch>on.module.css
              </span>
            </PickerRow>
          </PickerList>
          <PickerFoot>
            <PickerHint keys="⏎">open</PickerHint>
          </PickerFoot>
        </PickerFrame>
        <PickerFrame label="Go to symbol" narrow>
          <PickerInput lead="#">
            <input defaultValue="zzz" aria-label="Go to symbol" />
          </PickerInput>
          <PickerStatus>No matches</PickerStatus>
        </PickerFrame>
      </Specimen>

      <Specimen
        name="Menu and tooltip"
        source="Overlay.tsx › Menu, Tooltip"
        use={
          <>
            <strong>Menu</strong>: a context menu or a button&apos;s dropdown. Icons optional but
            aligned, shortcuts right in mono, the destructive item last, bold, filled black under the pointer, a group caption
            where there are more than seven items. <strong>Tooltip</strong>: inverted, short, with
            the shortcut if there is one.
          </>
        }
        ground="chrome"
      >
        <Line>
          <Cell label="menu">
            <Menu
              label="File"
              entries={[
                { kind: 'item', label: 'Open', icon: 'file', shortcut: 'Enter' },
                { kind: 'item', label: 'Open to the side', icon: 'columns-2', shortcut: 'Ctrl+Enter' },
                { kind: 'item', label: 'Copy path', icon: 'link' },
                { kind: 'separator' },
                { kind: 'group', label: 'Git' },
                { kind: 'item', label: 'Show history', icon: 'scroll-text' },
                { kind: 'item', label: 'Rollback', disabled: true },
                { kind: 'separator' },
                { kind: 'item', label: 'Move to trash', icon: 'trash-2', shortcut: 'Del', danger: true },
              ]}
            />
          </Cell>
          <Cell label="tooltip">
            <Line>
              <Tooltip text="Refresh" />
              <Tooltip text="Command palette" shortcut="Ctrl+Shift+P" />
            </Line>
          </Cell>
        </Line>
      </Specimen>
    </Chapter>
  )
}
