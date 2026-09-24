/**
 * Every scene the demo can render, in the order the feature site shows them.
 *
 * Import-free on purpose: `scripts/demo-shots.mjs` and `scripts/check-demo.mjs` read this list
 * as text, and `check:demo` fails when it and `SCENES` in `sceneTable.ts` disagree.
 */
export const SCENE_IDS = [
  'claude',
  'ide-diff',
  'editor',
  'git',
  'log',
  'merge',
  'agents',
  'tasks',
  'milestones',
  'harness',
  'models',
  'gitlab',
  'openspec',
  'docker',
  'search',
  'drawing',
  'settings-scheme',
  'keymap',
  'extensions',
  'new-project',
] as const

export type SceneId = (typeof SCENE_IDS)[number]
