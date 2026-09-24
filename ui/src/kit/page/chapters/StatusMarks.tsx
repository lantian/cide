import type { ReactElement } from 'react'

import { Avatar, Badge, Code, Counter, Dot, Kbd, Person, Tag } from '../../components/Status'
import { Cell, Chapter, Line, Specimen, Stack } from '../Specimen'

export function StatusMarks(): ReactElement {
  return (
    <Chapter
      id="status"
      title="Status marks"
      lead={
        <>
          Badges, tags, counters, dots, key hints, people and inline code. A tone is a meaning,
          not a colour picked for looks: <strong>blue</strong> in progress or informational,{' '}
          <strong>green</strong> done and fine, <strong>amber</strong> waiting on someone,{' '}
          <strong>red</strong> failed, <strong>purple</strong> merged, <strong>cyan</strong> a
          kind that is not a state, <strong>neutral</strong> no state worth a colour. The brand
          red accent is never a status — only &quot;yours&quot; or &quot;asking for you&quot;. The app has 7 separate
          tone vocabularies and 13 dot sizes today.
        </>
      }
    >
      <Specimen
        name="Badge"
        source="Status.tsx › Badge"
        use={
          <>
            A state someone else named — a pipeline, a job, an MR, a severity. Filled with the
            tone&apos;s gradient under a white label, lit along the top like the primary button.{' '}
            <strong>With a dot</strong> for a state; <strong>squared</strong> for a kind, so a row
            carrying both can tell them apart. <strong>soft</strong> (the hue on a wash) for a
            badge repeated down a long list, where thirty filled pills would outshout the titles.
          </>
        }
        ground="panel"
        spec={[
          ['size', '18px tall, 0 --sp-3, --fs-ui-11 600'],
          ['shape', 'state: --r-full with a 6px dot · kind: --r-1'],
          ['fill', '--grad-<tone> (both stops at least 4.5:1 under white), --on-fill, --fill-sheen'],
          ['soft', 'tone as text on color-mix(tone 12%, transparent)'],
        ]}
      >
        <Stack>
          <Line>
            <Badge tone="green" dot>
              passed
            </Badge>
            <Badge tone="blue" dot>
              running
            </Badge>
            <Badge tone="yellow" dot>
              pending
            </Badge>
            <Badge tone="red" dot>
              failed
            </Badge>
            <Badge dot>skipped</Badge>
          </Line>
          <Line>
            <Badge tone="green" squared icon="git-branch">
              Opened
            </Badge>
            <Badge tone="purple" squared icon="git-merge">
              Merged
            </Badge>
            <Badge squared>Closed</Badge>
            <Badge tone="red" squared>
              critical
            </Badge>
            <Badge tone="yellow" squared>
              minor
            </Badge>
            <Badge tone="cyan" squared>
              suggestion
            </Badge>
            <Badge tone="blue" squared>
              docs
            </Badge>
          </Line>
          <Line>
            <Badge tone="green" dot soft>
              passed
            </Badge>
            <Badge tone="blue" dot soft>
              running
            </Badge>
            <Badge tone="yellow" dot soft>
              pending
            </Badge>
            <Badge tone="red" dot soft>
              failed
            </Badge>
            <Badge tone="purple" squared soft>
              Merged
            </Badge>
          </Line>
        </Stack>
      </Specimen>

      <Specimen
        name="Tag"
        source="Status.tsx › Tag"
        use="A label the user attached or can remove — a task label, an active filter. Bordered, so it reads as an object rather than a state."
      >
        <Tag>frontend</Tag>
        <Tag tone="blue">docs</Tag>
        <Tag onRemove={() => {}}>assignee: developer</Tag>
        <Tag tone="accent" onRemove={() => {}}>
          milestone: M98
        </Tag>
      </Specimen>

      <Specimen
        name="Counter and dot"
        source="Status.tsx › Counter, Dot"
        use={
          <>
            <strong>Counter</strong>: a number beside a tab or title, filled like a badge; tabular
            figures, 99+ above 99; neutral by default, accent only when it asks for attention
            (awaiting input). <strong>Dot</strong>: a state with no words — unsaved, running —
            always with a label for its tooltip; a live state gets a halo one gap away.
          </>
        }
      >
        <Cell label="counter">
          <Line>
            <Counter value={3} />
            <Counter value={128} />
            <Counter value={2} tone="accent" />
            <Counter value={5} tone="green" />
          </Line>
        </Cell>
        <Cell label="dot">
          <Line>
            <Dot label="Unsaved" tone="accent" />
            <Dot label="Running" tone="blue" pulse />
            <Dot label="Waiting for input" tone="yellow" pulse />
            <Dot label="Failed" tone="red" />
            <Dot label="Idle" />
          </Line>
        </Cell>
      </Specimen>

      <Specimen
        name="Key hint, person, inline code"
        source="Status.tsx › Kbd, Avatar, Person, Code"
        use={
          <>
            <strong>Kbd</strong> wherever a shortcut is shown (menus, tooltips, empty states) —
            the app has no kbd element today. <strong>Person</strong> for an author or assignee,
            with an initial avatar tinted by role colour. <strong>Code</strong> for a branch,
            path or sha inside a sentence.
          </>
        }
      >
        <Cell label="kbd">
          <Line>
            <Kbd keys={['Ctrl', 'Shift', 'P']} />
            <Kbd keys={['Esc']} />
            <Kbd keys={['/']} />
          </Line>
        </Cell>
        <Cell label="person">
          <Line>
            <Person name="Ivan Vorontsov" tone="accent" />
            <Person name="developer" tone="blue" />
            <Avatar name="Reviewer Bot" tone="purple" />
          </Line>
        </Cell>
        <Cell label="code">
          <span style={{ fontSize: 'var(--fs-ui-12)' }}>
            Merge <Code>feature/ui-kit</Code> into <Code>master</Code> at <Code>f6832b6</Code>
          </span>
        </Cell>
      </Specimen>
    </Chapter>
  )
}
