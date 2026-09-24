import type { ReactElement } from 'react'

import { Button, IconButton } from '../../components/Button'
import { Cell, Chapter, Line, Specimen, Stack } from '../Specimen'

export function Buttons(): ReactElement {
  return (
    <Chapter
      id="buttons"
      title="Buttons"
      lead={
        <>
          Two heights — 30px (<code>md</code>, dialogs and forms) and 24px (<code>sm</code>, panel
          rows and toolbars) — and five variants. Today the app has about 45 separately styled
          secondary buttons in 3 families of sizes; every one of them becomes a{' '}
          <code>secondary</code> or <code>quiet</code> here.
        </>
      }
    >
      <Specimen
        name="Variants"
        source="Button.tsx › Button"
        use={
          <>
            <strong>primary</strong> — the one act the surface exists for; at most one per footer
            or row. <strong>secondary</strong> — every other act beside it.{' '}
            <strong>quiet</strong> — an act inside dense content. <strong>danger</strong> — the
            confirming half of something destructive: filled black, like the primary in ink, so
            destroying never shares the primary&apos;s red.{' '}
            <strong>link</strong> — an act inside a sentence.
          </>
        }
        ground="chrome"
        spec={[
          ['height', 'md 30px · sm 24px (× --ui-scale)'],
          ['padding', 'md 0 --sp-6 (16) · sm 0 --sp-4 (8)'],
          ['radius', '--r-2 (6)'],
          ['label', 'md --fs-ui-12 · sm --fs-ui-11, weight 400 (primary 600)'],
          ['primary', '--grad-accent-ink, --on-accent, --shadow-1 → --shadow-2 on hover'],
          ['secondary', '--panel, 1px --border; hover --panel-2 with --accent-dim border'],
          ['focus', '--focus-ring, 2px offset'],
          ['disabled', 'primary/danger opacity .45; secondary/quiet --faint label'],
        ]}
      >
        <Stack>
          <Line>
            <Button variant="primary" icon="check">
              Continue
            </Button>
            <Button>Back</Button>
            <Button variant="quiet">Skip</Button>
            <Button variant="danger" icon="trash-2">
              Delete branch
            </Button>
            <Button variant="link">Open settings</Button>
          </Line>
          <Line>
            <Button variant="primary" size="sm">
              Approve
            </Button>
            <Button size="sm" icon="refresh-cw">
              Refresh
            </Button>
            <Button variant="quiet" size="sm">
              Load more
            </Button>
            <Button variant="danger" size="sm">
              Discard
            </Button>
          </Line>
        </Stack>
      </Specimen>

      <Specimen
        name="States"
        source="Button.tsx › Button"
        use="Busy replaces the leading icon with a turning loader and disables the button, so an in-flight act cannot be sent twice."
        ground="chrome"
      >
        <Cell label="rest">
          <Button variant="primary">Create project</Button>
        </Cell>
        <Cell label="busy">
          <Button variant="primary" busy>
            Creating…
          </Button>
        </Cell>
        <Cell label="disabled">
          <Button variant="primary" disabled>
            Create project
          </Button>
        </Cell>
        <Cell label="secondary disabled">
          <Button disabled>Back</Button>
        </Cell>
        <Cell label="with menu">
          <Button trailingIcon="chevron-down">Pull</Button>
        </Cell>
      </Specimen>

      <Specimen
        name="Icon button"
        source="Button.tsx › IconButton"
        use={
          <>
            Tools in a panel header, a toolbar, a row. No border at rest, a <code>--panel-2</code>{' '}
            wash on hover; a toggle that is on takes an accent wash. The label is required — it
            is the control&apos;s only name and its tooltip. More than three in a header → put the
            rest in a <code>…</code> menu.
          </>
        }
        spec={[
          ['size', 'sm 24×24 (icon 14) · md 30×30 (icon 16)'],
          ['radius', '--r-1 (4)'],
          ['rest / hover', '--dim on transparent → --text on --panel-2'],
          ['pressed', '--accent on accent 12% wash'],
        ]}
      >
        <Line>
          <IconButton icon="refresh-cw" label="Refresh" />
          <IconButton icon="plus" label="New task" />
          <IconButton icon="list-tree" label="Group by folder" pressed />
          <IconButton icon="ellipsis" label="More actions" />
          <IconButton icon="x" label="Close" />
          <IconButton icon="settings" label="Settings" size="md" />
          <IconButton icon="trash-2" label="Delete" disabled />
        </Line>
      </Specimen>
    </Chapter>
  )
}
