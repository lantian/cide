import { useState, type ReactElement } from 'react'

import { IconButton } from '../../components/Button'
import {
  ChromeButton,
  ChromeTab,
  ChromeTabs,
  PaneBar,
  Rail,
  RailButton,
  StatusBar,
  StatusItem,
  WindowControls,
} from '../../components/Chrome'
import { Cell, Chapter, Line, Specimen } from '../Specimen'

export function ChromeChapter(): ReactElement {
  const [panel, setPanel] = useState('git')
  const [tab, setTab] = useState('Kit.tsx')
  const [wrap, setWrap] = useState(false)
  return (
    <Chapter
      id="chrome"
      title="App chrome"
      lead="The rail, the tab strip, the status bar, a pane's title bar and the window's own buttons. The app's chrome components keep their own markup — drag, overflow, detach and the layout audit read it — and compose these classes, so the drawing lives here."
    >
      <Specimen
        name="Rail"
        source="Chrome.tsx › Rail, RailButton"
        use="One button per sidebar panel. The current panel is the accent on a 12% wash, like every icon toggle; a count is the kit's Counter — neutral, accent only when it asks for you, red for failures."
        spec={[
          ['button', '32×32 in the 46px --h-rail, --r-3; hover --panel-2'],
          ['count', 'Counter, top-right, 99+ above 99'],
        ]}
      >
        <Rail label="Panels">
          <RailButton icon="file" label="Explorer" current={panel === 'files'} onClick={() => setPanel('files')} />
          <RailButton
            icon="git-branch"
            label="Git"
            count={12}
            current={panel === 'git'}
            onClick={() => setPanel('git')}
          />
          <RailButton icon="search" label="Search" current={panel === 'search'} onClick={() => setPanel('search')} />
          <RailButton icon="circle-alert" label="Problems" count={3} countTone="red" onClick={() => setPanel('problems')} />
          <RailButton icon="square-terminal" label="Tasks" count={2} countTone="accent" onClick={() => setPanel('tasks')} />
        </Rail>
      </Specimen>

      <Specimen
        name="Tab strip"
        source="Chrome.tsx › ChromeTabs, ChromeTab"
        use="The editor's tabs and a detached window's. The current tab is the panel ground with the accent underline and --text-hi (not bolder: the strip's widths are measured and must not reflow); an unsaved tab's close wears the accent dot until it is pointed at."
        spec={[
          ['strip', '--h-tabstrip on --panel-2, 1px --border below'],
          ['tab', 'hover --panel; current --panel + 2px accent underline, --text-hi'],
          ['close', '20×20, --r-1; the dirty dot 8px accent'],
        ]}
      >
        <Cell label="three tabs" grow>
          <ChromeTabs label="Editor tabs">
            {['Kit.tsx', 'Chrome.module.css', 'ui-kit.md'].map((t) => (
              <ChromeTab
                key={t}
                title={t}
                icon="file-code"
                current={tab === t}
                dirty={t === 'Chrome.module.css'}
                onSelect={() => setTab(t)}
                onClose={() => {}}
              />
            ))}
          </ChromeTabs>
        </Cell>
      </Specimen>

      <Specimen
        name="Pane bar and chrome buttons"
        source="Chrome.tsx › PaneBar, ChromeButton, WindowControls"
        use="A pane's title bar, lit when the pane has focus, with its own 22px buttons; the window's minimise, maximise and close for a frame the app draws itself. Close is red under the pointer only."
        spec={[
          ['bar', '28 × scale, --panel-2; focused --panel and --text-hi'],
          ['button', '22×22, --r-2; hover --panel-2; on = accent on a 12% wash'],
        ]}
      >
        <Line>
          <Cell label="focused" grow>
            <PaneBar
              title="claude — cide"
              icon="square-terminal"
              focused
              tools={
                <>
                  <ChromeButton icon="columns-2" label="Wrap" on={wrap} onClick={() => setWrap(!wrap)} />
                  <ChromeButton icon="x" label="Close pane" />
                </>
              }
            />
          </Cell>
          <Cell label="at rest" grow>
            <PaneBar title="bash" icon="square-terminal" tools={<IconButton icon="x" label="Close pane" />} />
          </Cell>
          <Cell label="window">
            <WindowControls />
          </Cell>
        </Line>
      </Specimen>

      <Specimen
        name="Status bar"
        source="Chrome.tsx › StatusBar, StatusItem"
        use="Facts about the window, mono at 12px: the branch, the language, the caret. An item that does something is a button with the kit's hover; a status hue goes on the icon only, the words stay --dim."
        spec={[['bar', '--h-status on --chrome, 1px --border above, mono 12, clips; items 20 × scale, --r-1']]}
      >
        <Cell label="bar" grow>
          <StatusBar>
            <StatusItem icon="git-branch" tone="purple" onClick={() => {}}>
              master
            </StatusItem>
            <StatusItem icon="circle-check" tone="green">
              No problems
            </StatusItem>
            <StatusItem>Ln 12, Col 4</StatusItem>
          </StatusBar>
        </Cell>
      </Specimen>
    </Chapter>
  )
}
