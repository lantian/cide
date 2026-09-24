import type { ReactElement } from 'react'

import { Button } from '../../components/Button'
import {
  Banner,
  Checklist,
  EmptyState,
  Note,
  Progress,
  Skeleton,
  Spinner,
  Toast,
} from '../../components/Feedback'
import { Cell, Chapter, Frame, Line, Specimen, Stack } from '../Specimen'

export function FeedbackChapter(): ReactElement {
  return (
    <Chapter
      id="feedback"
      title="Feedback"
      lead={
        <>
          How the app says what happened. Drawn like the filled marks: the outcome&apos;s gradient
          in a bar down the left and behind a round icon mark, a fading wash, the words in the
          ordinary text colour so they read the same in every tone. The app
          has about 10 left-border callouts, 15 inline error styles, 5 pane notices and 7
          duplicate spinner animations today.
        </>
      }
    >
      <Specimen
        name="Note"
        source="Feedback.tsx › Note"
        use={
          <>
            Inline, inside a form or a card, about the thing next to it.{' '}
            <strong>info</strong> neutral context, <strong>ok</strong> a check passed,{' '}
            <strong>warn</strong> it will work but mind this, <strong>bad</strong> it will not
            work. At most one action, at the right.
          </>
        }
        ground="chrome"
        spec={[
          ['box', '--r-2, 1px border tone 24%, wash tone 11% → 3% left to right'],
          ['bar', '3px down the left edge, --grad-<tone>'],
          ['mark', '18px disc, --grad-<tone>, white 12px glyph'],
          ['text', '--fs-ui-12, 1.45, --text; title 600 --text-hi'],
        ]}
      >
        <Stack>
          <Note tone="ok">The folder is empty and writable.</Note>
          <Note tone="warn" title="The folder is not empty">
            Existing files are kept; the project is created around them.
          </Note>
          <Note tone="bad" action={<Button size="sm">Choose another</Button>}>
            You do not have permission to write to /opt/projects.
          </Note>
          <Note tone="info">Roles take tasks from the board and work in their own worktree.</Note>
        </Stack>
      </Specimen>

      <Specimen
        name="Banner and toast"
        source="Feedback.tsx › Banner, Toast"
        use={
          <>
            <strong>Banner</strong>: flush across the top of a panel or pane, about the whole of
            it (&quot;New commits available&quot;). <strong>Toast</strong>: something that happened
            elsewhere, bottom-right, dismissable; a failure toast stays until dismissed.
          </>
        }
      >
        <Frame>
          <Banner action={<Button variant="link">Refresh</Button>}>New commits available.</Banner>
          <Banner tone="warn">The remote is unreachable; showing cached data.</Banner>
          <div style={{ height: 'calc(60px * var(--ui-scale))' }} />
        </Frame>
        <Stack>
          <Toast tone="ok" title="Pushed to origin/master">
            3 commits.
          </Toast>
          <Toast tone="bad" title="Push rejected" action={<Button size="sm">Pull</Button>}>
            The remote has commits you do not have.
          </Toast>
        </Stack>
      </Specimen>

      <Specimen
        name="Empty state"
        source="Feedback.tsx › EmptyState"
        use="A claim, a sentence, one button, centred both ways in the panel. A lone button at the top of an empty column reads as a list that failed to load."
        ground="panel"
      >
        <Frame>
          <EmptyState
            icon="git-merge"
            title="No merge requests"
            action={<Button variant="primary" size="sm">Connect GitLab</Button>}
          >
            Merge requests assigned to you or waiting for your review appear here.
          </EmptyState>
        </Frame>
        <Frame>
          <EmptyState icon="search" title="Nothing matches “kit”">
            Try fewer words, or clear the filter.
          </EmptyState>
        </Frame>
      </Specimen>

      <Specimen
        name="Progress, spinner, skeleton"
        source="Feedback.tsx › Progress, Spinner, Skeleton"
        use="A bar when the fraction is known, a spinner when it is not, skeleton lines while a list's first page loads. The spinner pauses under reduced motion."
      >
        <Cell label="progress" grow>
          <Stack>
            <Progress value={0.62} label="Indexing" />
            <Progress value={1} label="Tasks done" tone="ok" />
          </Stack>
        </Cell>
        <Cell label="spinner">
          <Spinner label="Loading pipelines…" />
        </Cell>
        <Cell label="skeleton" grow>
          <Stack>
            <Skeleton width="80%" />
            <Skeleton width="55%" />
            <Skeleton width="68%" />
          </Stack>
        </Cell>
      </Specimen>

      <Specimen
        name="Checklist"
        source="Feedback.tsx › Checklist"
        use="Several steps that run in order and each succeed or fail — the wizard's Create step, a verify run. The failing step says why under itself."
        ground="chrome"
      >
        <Line>
          <div style={{ width: 'calc(420px * var(--ui-scale))' }}>
            <Checklist
              items={[
                { label: 'Create the folder', state: 'done' },
                { label: 'Initialise git', state: 'done', detail: 'On branch main.' },
                { label: 'Write the roles', state: 'running' },
                {
                  label: 'Open the project',
                  state: 'waiting',
                },
              ]}
            />
          </div>
          <div style={{ width: 'calc(420px * var(--ui-scale))' }}>
            <Checklist
              items={[
                { label: 'Create the folder', state: 'done' },
                {
                  label: 'Clone the repository',
                  state: 'failed',
                  detail: 'Permission denied (publickey).',
                },
              ]}
            />
          </div>
        </Line>
      </Specimen>
    </Chapter>
  )
}
