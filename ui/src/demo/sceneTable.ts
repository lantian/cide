/**
 * One entry per feature the site shows, one module per scene under `scenes/`. `check:demo` holds
 * this table and `SCENE_IDS` to the same keys — the `Record<SceneId, Scene>` type does half of
 * that, the check does the other half (the capture script reads the ids as text).
 */
import type { Scene } from './scenes'
import type { SceneId } from './sceneIds'
import { claude } from './scenes/claude'
import { ideDiff } from './scenes/ide-diff'
import { editor } from './scenes/editor'
import { git } from './scenes/git'
import { log } from './scenes/log'
import { merge } from './scenes/merge'
import { agents } from './scenes/agents'
import { tasks } from './scenes/tasks'
import { milestones } from './scenes/milestones'
import { harness } from './scenes/harness'
import { models } from './scenes/models'
import { gitlab } from './scenes/gitlab'
import { openspec } from './scenes/openspec'
import { docker } from './scenes/docker'
import { search } from './scenes/search'
import { drawing } from './scenes/drawing'
import { settingsScheme } from './scenes/settings-scheme'
import { keymap } from './scenes/keymap'
import { extensions } from './scenes/extensions'
import { newProject } from './scenes/new-project'

export const SCENES: Record<SceneId, Scene> = {
  'claude': claude,
  'ide-diff': ideDiff,
  'editor': editor,
  'git': git,
  'log': log,
  'merge': merge,
  'agents': agents,
  'tasks': tasks,
  'milestones': milestones,
  'harness': harness,
  'models': models,
  'gitlab': gitlab,
  'openspec': openspec,
  'docker': docker,
  'search': search,
  'drawing': drawing,
  'settings-scheme': settingsScheme,
  'keymap': keymap,
  'extensions': extensions,
  'new-project': newProject,
}
