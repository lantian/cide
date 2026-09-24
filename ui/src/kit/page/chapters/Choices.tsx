import { useState, type ReactElement } from 'react'

import { Icon } from '@/icons/Icon'
import {
  Checkbox,
  ChoiceCards,
  RadioGroup,
  Segmented,
  Switch,
  ToggleCard,
} from '../../components/Choice'
import { Cell, Chapter, Specimen, Stack } from '../Specimen'

export function Choices(): ReactElement {
  const [a, setA] = useState(true)
  const [b, setB] = useState(false)
  const [mode, setMode] = useState<'merge' | 'rebase' | 'ff'>('rebase')
  const [on, setOn] = useState(true)
  const [scope, setScope] = useState<'assigned' | 'reviewing' | 'created'>('reviewing')
  const [view, setView] = useState<'unified' | 'split'>('unified')
  const [kind, setKind] = useState<'empty' | 'clone' | 'template'>('clone')
  const [git, setGit] = useState(true)
  return (
    <Chapter
      id="choices"
      title="Choices"
      lead={
        <>
          Checkbox, radio, switch, segmented control and card. The app draws &quot;on&quot; five
          different ways today (chrome-hi, accent text, accent fill, <code>--sel</code>,{' '}
          <code>--accent-dim</code>); the kit has one per control, below.
        </>
      }
    >
      <Specimen
        name="Checkbox and radio"
        source="Choice.tsx › Checkbox, RadioGroup"
        use={
          <>
            Native inputs with <code>accent-color</code>, never redrawn boxes.{' '}
            <strong>Checkbox</strong>: an independent yes/no applied on submit.{' '}
            <strong>Radio</strong>: one of 2–5 options that each need a sentence.
          </>
        }
      >
        <Stack>
          <Checkbox label="Stage all files" checked={a} onChange={setA} />
          <Checkbox
            label="Sign the commit"
            hint="Uses the key in user.signingkey."
            checked={b}
            onChange={setB}
          />
          <Checkbox label="Some files staged" checked={false} indeterminate onChange={() => {}} />
          <Checkbox label="Push tags (no remote)" checked={false} disabled onChange={() => {}} />
        </Stack>
        <RadioGroup
          legend="When the branch has diverged"
          name="pull"
          value={mode}
          onChange={setMode}
          options={[
            { value: 'merge', label: 'Merge', hint: 'Keeps both histories; adds a merge commit.' },
            { value: 'rebase', label: 'Rebase', hint: 'Replays your commits on top.' },
            { value: 'ff', label: 'Fast-forward only', hint: 'Refuses when it cannot.' },
          ]}
        />
        <Cell label="inline">
          <RadioGroup
            legend="Reset mode"
            name="reset"
            inline
            value={mode}
            onChange={setMode}
            options={[
              { value: 'merge', label: 'Soft' },
              { value: 'rebase', label: 'Mixed' },
              { value: 'ff', label: 'Hard' },
            ]}
          />
        </Cell>
      </Specimen>

      <Specimen
        name="Switch"
        source="Choice.tsx › Switch"
        use="A yes/no that takes effect the moment it flips — settings. On is the primary gradient, so a switch that is on reads as the same family as the button it replaces."
        spec={[
          ['track', '32×18, --r-full; off --chrome-hi + --border; on --grad-accent-ink'],
          ['knob', '12×12, --panel with --shadow-1; on → --on-accent'],
        ]}
      >
        <Cell label="on">
          <Switch label="Enabled" checked={on} onChange={setOn} />
        </Cell>
        <Cell label="off">
          <Switch label="Disabled" checked={!on} onChange={(v) => setOn(!v)} />
        </Cell>
        <Cell label="disabled">
          <Switch label="Locked" checked disabled onChange={() => {}} />
        </Cell>
      </Specimen>

      <Specimen
        name="Segmented control"
        source="Choice.tsx › Segmented"
        use={
          <>
            One of 2–4 short options that are views or scopes of the same thing — the GitLab
            inbox&apos;s scope tabs. A sunken trough, the chosen segment raised onto{' '}
            <code>--panel</code> with a shadow, its label dark and a step heavier. Arrow keys move. Not for switching
            to different content: that is Tabs.
          </>
        }
        ground="panel"
        spec={[
          ['trough', '--panel-2, 1px --border-soft, --r-2, 2px padding'],
          ['segment', 'md 26px · sm 20px, --fs-ui-12/11, --dim'],
          ['chosen', '--panel, 1px --border, --shadow-1, --text-hi, 500'],
        ]}
      >
        <Cell label="block (panel filter)" grow>
          <Segmented
            label="Scope"
            block
            value={scope}
            onChange={setScope}
            options={[
              { value: 'assigned', label: 'Assigned' },
              { value: 'reviewing', label: 'Reviewing' },
              { value: 'created', label: 'Created' },
            ]}
          />
        </Cell>
        <Cell label="sm, with icons">
          <Segmented
            label="Diff view"
            size="sm"
            value={view}
            onChange={setView}
            options={[
              { value: 'unified', label: 'Unified', icon: 'rows-2' },
              { value: 'split', label: 'Split', icon: 'columns-2' },
            ]}
          />
        </Cell>
      </Specimen>

      <Specimen
        name="Choice card and toggle card"
        source="Choice.tsx › ChoiceCards, ToggleCard"
        use="The New Project wizard's two big choices. Cards: one of 2–4 options that each need a picture or a paragraph. Toggle card: a checkbox that needs a sentence of why."
        ground="chrome"
        spec={[
          ['card', '--panel, 1px --border, --r-3; hover --accent-dim + --shadow-2'],
          ['chosen', '--accent border + 1px accent ring, accent 5% wash, check dot'],
          ['art', '88px, --grad-accent-soft on --panel-2'],
        ]}
      >
        <Stack>
          <ChoiceCards
            label="Project type"
            value={kind}
            onChange={setKind}
            options={[
              {
                value: 'empty',
                title: 'Empty',
                text: 'A folder and a git repository, nothing else.',
                art: <Icon name="folder" size={3} />,
              },
              {
                value: 'clone',
                title: 'Clone',
                text: 'Start from an existing repository.',
                art: <Icon name="git-branch" size={3} />,
              },
              {
                value: 'template',
                title: 'With subagents',
                text: 'A project with roles that take tasks from the board.',
                art: <Icon name="blocks" size={3} />,
              },
            ]}
          />
          <ToggleCard
            title="Initialise a git repository"
            hint="Creates the first commit with a .gitignore for the chosen stack."
            checked={git}
            onChange={setGit}
          />
        </Stack>
      </Specimen>
    </Chapter>
  )
}
