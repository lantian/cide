/*
 * Everything the site says, in both languages, in one place.
 *
 * The page's structure lives in index.html and app.js renders the feature chapters from
 * CHAPTERS below. A translation is one more key beside `en` and `ru` — nothing in app.js names a
 * language. Keep the two in step: app.js falls back to English per string, so a missing Russian
 * line shows up as English rather than as a blank, which is easy to miss when reading only one.
 *
 * Screenshots are `img/<shot>-<theme>.webp`, rendered from the real UI by
 * `pnpm --dir ui demo:shots` (ui/scripts/demo-shots.mjs). A shot id here must be a scene id in
 * ui/src/demo/sceneIds.ts.
 */
window.CIDE_SITE = {
  ui: {
    en: {
      'nav.features': 'Features',
      'nav.install': 'Install',
      'nav.github': 'GitHub',
      'theme.toDark': 'Switch to dark theme',
      'theme.toLight': 'Switch to light theme',
      'hero.eyebrow': 'Open source · Linux-first · Rust + Tauri',
      'hero.title.1': 'The IDE built around',
      'hero.title.2': 'Claude Code',
      'hero.lead':
        'cide puts a live Claude Code session at the centre of the window — a pinned tab with a tiling grid of conversations, shells and diffs — and surrounds it with a real IDE: language servers, IDEA-style git, and a team of agents that work in their own worktrees.',
      'hero.cta.primary': 'Get cide on GitHub',
      'hero.cta.secondary': 'Tour the features',
      'hero.caption': 'The real cide UI — every picture on this page is rendered from it.',
      'pillars.1.title': 'Claude at the centre',
      'pillars.1.body':
        'Not a terminal that happens to run Claude. cide is a Claude Code IDE: edits arrive as diffs, diagnostics come from its language servers, sessions outlive every window.',
      'pillars.2.title': 'Agents that ship',
      'pillars.2.body':
        'Hand work to roles. Each task runs in its own git worktree on its own branch, reports back on the board, and is merged only after it passes your verify command.',
      'pillars.3.title': 'A real IDE around it',
      'pillars.3.body':
        'CodeMirror with real language servers, changelists and hunk staging, a log with graph lanes, a three-pane merge, GitLab review, Docker and OpenSpec.',
      'more.title': 'And the rest of an IDE',
      'more.1': 'Tear any pane or tab into its own window — the session carries on',
      'more.2': 'One window per project, or all projects as tabs',
      'more.3': 'Project tabs show what is running inside them',
      'more.4': 'Paste an image straight into the project; images open in a viewer',
      'more.5': 'Blame, file history and an outline',
      'more.6': 'Shortcuts that work under non-Latin keyboard layouts',
      'more.7': 'Proxy settings and Linux graphics fixes, each explained in a sentence',
      'more.8': 'A headless binary to inspect every piece of state',
      'install.title': 'Install',
      'install.lead':
        'Download a build for your system from the latest GitHub release: an AppImage, .deb or tarball for Linux, a .dmg for macOS. You also need the claude CLI, installed and signed in. cide runs Claude; it does not ship it.',
      'install.download': 'Download the latest release',
      'install.all': 'All releases',
      'install.source': 'Or build it from source',
      'install.sourceLead': 'You need Rust, Node with pnpm and WebKitGTK 4.1.',
      'install.copy': 'Copy',
      'install.copied': 'Copied',
      'install.note':
        'A source build uses rust-analyzer and gopls from your PATH. ./build.sh makes an AppImage that carries its own language servers.',
      'install.more': 'Full build and packaging guide',
      'footer.licence': 'MIT licensed',
      'footer.made': 'Built with Claude Code, in cide.',
      'lightbox.close': 'Close',
    },
    ru: {
      'nav.features': 'Возможности',
      'nav.install': 'Установка',
      'nav.github': 'GitHub',
      'theme.toDark': 'Тёмная тема',
      'theme.toLight': 'Светлая тема',
      'hero.eyebrow': 'Открытый код · в первую очередь Linux · Rust + Tauri',
      'hero.title.1': 'IDE, построенная вокруг',
      'hero.title.2': 'Claude Code',
      'hero.lead':
        'В центре окна cide — живая сессия Claude Code: закреплённая вкладка с сеткой разговоров, терминалов и диффов. Вокруг неё — настоящая IDE: языковые серверы, git в стиле IDEA и команда агентов, каждый из которых работает в своём worktree.',
      'hero.cta.primary': 'cide на GitHub',
      'hero.cta.secondary': 'Смотреть возможности',
      'hero.caption': 'Настоящий интерфейс cide — все картинки на этой странице сняты с него.',
      'pillars.1.title': 'Claude в центре',
      'pillars.1.body':
        'Это не терминал, в котором случайно запущен Claude. cide — IDE для Claude Code: правки приходят диффами, диагностика — от языковых серверов, а сессии переживают любое окно.',
      'pillars.2.title': 'Агенты, которые доводят до конца',
      'pillars.2.body':
        'Раздавайте работу ролям. Каждая задача идёт в своём git worktree на своей ветке, отчитывается на доске и вливается только после вашей команды проверки.',
      'pillars.3.title': 'Настоящая IDE вокруг',
      'pillars.3.body':
        'CodeMirror с настоящими языковыми серверами, changelists и поблочное индексирование, лог с графом веток, трёхпанельное слияние, ревью в GitLab, Docker и OpenSpec.',
      'more.title': 'И всё остальное, что ждёшь от IDE',
      'more.1': 'Любую панель или вкладку можно вынести в отдельное окно — сессия продолжится',
      'more.2': 'Окно на каждый проект или все проекты вкладками',
      'more.3': 'Вкладка проекта показывает, что в нём сейчас работает',
      'more.4': 'Вставьте картинку прямо в проект — изображения открываются в просмотрщике',
      'more.5': 'Blame, история файла и структура',
      'more.6': 'Горячие клавиши работают в любой раскладке, включая русскую',
      'more.7': 'Настройки прокси и обходы графики Linux — каждый объяснён одной фразой',
      'more.8': 'Headless-утилита, чтобы заглянуть в любое состояние',
      'install.title': 'Установка',
      'install.lead':
        'Скачайте сборку для своей системы из последнего релиза на GitHub: AppImage, .deb или архив для Linux, .dmg для macOS. Ещё нужен установленный и авторизованный CLI claude. cide запускает Claude, но не поставляет его.',
      'install.download': 'Скачать последний релиз',
      'install.all': 'Все релизы',
      'install.source': 'Или соберите из исходников',
      'install.sourceLead': 'Нужны Rust, Node с pnpm и WebKitGTK 4.1.',
      'install.copy': 'Копировать',
      'install.copied': 'Скопировано',
      'install.note':
        'Сборка из исходников берёт rust-analyzer и gopls из PATH. ./build.sh собирает AppImage со своими языковыми серверами.',
      'install.more': 'Полное руководство по сборке и упаковке',
      'footer.licence': 'Лицензия MIT',
      'footer.made': 'Сделано с Claude Code, в cide.',
      'lightbox.close': 'Закрыть',
    },
  },

  chapters: [
    {
      id: 'claude',
      en: {
        title: 'Claude at the centre',
        lead: 'Every project opens on the session, not on a file.',
      },
      ru: {
        title: 'Claude в центре',
        lead: 'Каждый проект открывается на сессии, а не на файле.',
      },
      features: [
        {
          shot: 'claude',
          en: {
            title: 'A Claude tab that never closes',
            body: 'Every project has a pinned Claude tab holding a tiling grid of Claude sessions, shells and read-only diffs. Sessions belong to cide’s core, not to a window: close a pane, close the window, quit the app, and the conversation is still there on the next launch.',
            points: [
              'Split, resize and drag panes between rows and columns',
              'Sessions survive closed panes, closed windows and restarts',
              'Send a selection to a named conversation',
              'Paths and task ids in the scrollback open when clicked',
            ],
          },
          ru: {
            title: 'Вкладка Claude, которая не закрывается',
            body: 'У каждого проекта есть закреплённая вкладка Claude с сеткой из сессий Claude, терминалов и диффов только для чтения. Сессии принадлежат ядру cide, а не окну: закройте панель, окно или всё приложение — разговор будет на месте при следующем запуске.',
            points: [
              'Разделяйте панели, меняйте их размер и перетаскивайте между строками и столбцами',
              'Сессии переживают закрытие панелей, окон и перезапуск',
              'Выделенный текст можно отправить в нужный разговор',
              'Пути и номера задач в выводе открываются по клику',
            ],
          },
        },
        {
          shot: 'ide-diff',
          en: {
            title: 'A real Claude Code IDE, not a terminal wrapper',
            body: 'cide runs Claude Code’s IDE-integration MCP server, so Claude treats cide the way it treats VS Code or JetBrains: proposed edits open as diffs in cide’s own editor, and Claude’s questions about your code are answered from state cide already holds.',
            points: [
              'Edits arrive as reviewable diffs, not as surprises on disk',
              'getDiagnostics is answered by the language server cide is already running',
              'Ctrl+G opens Claude’s plan in a cide tab',
            ],
          },
          ru: {
            title: 'Настоящая IDE для Claude Code, а не обёртка над терминалом',
            body: 'cide запускает MCP-сервер интеграции Claude Code с IDE, поэтому Claude работает с cide так же, как с VS Code или JetBrains: предложенные правки открываются диффами в редакторе cide, а на вопросы о коде отвечает то, что cide уже знает.',
            points: [
              'Правки приходят диффами на ревью, а не сюрпризом на диске',
              'getDiagnostics отвечает уже запущенный языковой сервер',
              'Ctrl+G открывает план Claude во вкладке cide',
            ],
          },
        },
      ],
    },
    {
      id: 'agents',
      en: {
        title: 'A team of agents',
        lead: 'Claude plans, and roles carry the work out in parallel, each on its own branch.',
      },
      ru: {
        title: 'Команда агентов',
        lead: 'Claude планирует, а роли параллельно выполняют работу — каждая на своей ветке.',
      },
      features: [
        {
          shot: 'agents',
          en: {
            title: 'Roles that work in their own worktrees',
            body: 'Describe a role once — a coder, a reviewer, a tester, or a Claude Code subagent you already have — and assign it a task. It starts in its own git worktree on its own branch, you watch it live, and finished work is merged back from the panel.',
            points: [
              'Each run gets a checkout under .cide/worktrees/, so parallel runs don’t collide',
              'A colour per role, and a clock that counts only working time',
              'Pause, resume, stop, or open the run’s own terminal',
              'Integrate a finished run with one click',
            ],
          },
          ru: {
            title: 'Роли работают в своих worktree',
            body: 'Опишите роль один раз — кодер, ревьюер, тестировщик или уже существующий субагент Claude Code — и назначьте ей задачу. Она стартует в своём git worktree на своей ветке, вы наблюдаете за ней вживую, а готовая работа вливается из панели.',
            points: [
              'Своя копия в .cide/worktrees/ — параллельные запуски не мешают друг другу',
              'Свой цвет у каждой роли и часы, которые считают только рабочее время',
              'Пауза, продолжение, остановка или собственный терминал запуска',
              'Готовый запуск вливается одним нажатием',
            ],
          },
        },
        {
          shot: 'tasks',
          en: {
            title: 'A task tracker Claude can call',
            body: 'The board lives in your repository as .cide/tasks.json, and Claude reads and writes it over MCP with the cide_task_* tools. Assigning a task to a role starts that role. Runs report back as comments, signed in the role’s colour.',
            points: [
              'Typed links: blocked-by, subtask-of, related',
              'Screenshots and files attached to tasks and comments',
              'Mention a role in a comment to hand the task back to it',
              'Plain JSON you can diff, review and merge',
            ],
          },
          ru: {
            title: 'Трекер задач, к которому обращается Claude',
            body: 'Доска хранится в репозитории как .cide/tasks.json, и Claude читает и пишет её через MCP-инструменты cide_task_*. Назначили задачу роли — роль начала работу. Отчёты приходят комментариями, подписанными цветом роли.',
            points: [
              'Типизированные связи: «блокируется», «подзадача», «связана»',
              'Скриншоты и файлы в задачах и комментариях',
              'Упомяните роль в комментарии — и задача вернётся к ней',
              'Обычный JSON: его можно сравнивать, ревьюить и сливать',
            ],
          },
        },
        {
          shot: 'milestones',
          en: {
            title: 'Milestones with a gate',
            body: 'Group tasks into milestones and let them run. Nothing merges until your verify command passes, done is refused while a branch is unmerged, and anything that needs you goes to the inbox — so you check milestones instead of every task.',
            points: [
              'A verify command runs before every merge',
              'A finished run opens its own reviewer, from a per-project template',
              'An inbox for the decisions that are really yours',
            ],
          },
          ru: {
            title: 'Вехи с контрольной точкой',
            body: 'Объединяйте задачи в вехи и запускайте их. Ничего не вливается, пока не прошла ваша команда проверки; задачу нельзя закрыть, пока её ветка не влита; всё, что требует вашего решения, попадает во входящие. Вы проверяете вехи, а не каждую задачу.',
            points: [
              'Команда проверки запускается перед каждым слиянием',
              'Завершённый запуск сам открывает ревьюера по шаблону проекта',
              'Входящие — для решений, которые действительно за вами',
            ],
          },
        },
        {
          shot: 'harness',
          en: {
            title: 'Claude Code, opencode, Codex, MiMo',
            body: 'Claude Code is the default, and each role can run on another harness. Opening a run shows that harness’s own interface with its full history, whichever one it is.',
            points: [
              'A harness and a model per role',
              'Codex can also be the main console',
              'The same board, worktrees and review whichever harness runs',
            ],
          },
          ru: {
            title: 'Claude Code, opencode, Codex, MiMo',
            body: 'По умолчанию — Claude Code, но каждая роль может работать на другой среде исполнения. Открыв запуск, вы увидите родной интерфейс этой среды со всей историей.',
            points: [
              'Своя среда и модель для каждой роли',
              'Codex может быть и главной консолью',
              'Та же доска, те же worktree и то же ревью — для любой среды',
            ],
          },
        },
        {
          shot: 'models',
          en: {
            title: 'Choose what your agents run on',
            body: 'Add providers and models, group them into pools with a running limit per entry and automatic failover, and override them per role. The orchestrating Claude can change all of it itself over MCP.',
            points: [
              'Pools with per-entry concurrency and failover',
              'Shows why a run landed on a particular entry',
              'Context fill, and which model ran which line',
            ],
          },
          ru: {
            title: 'Выбирайте, на чём работают агенты',
            body: 'Добавляйте провайдеров и модели, собирайте их в пулы с лимитом одновременных запусков на каждую запись и автоматическим переключением, переопределяйте их для отдельных ролей. Управляющий Claude может менять всё это сам через MCP.',
            points: [
              'Пулы с лимитом параллельных запусков и переключением при сбое',
              'Видно, почему запуск попал на конкретную запись',
              'Заполненность контекста и какая модель что сделала',
            ],
          },
        },
      ],
    },
    {
      id: 'ide',
      en: {
        title: 'A real IDE',
        lead: 'The editor, search and git you expect, and all of them work.',
      },
      ru: {
        title: 'Настоящая IDE',
        lead: 'Редактор, поиск и git, которых вы ждёте, — и всё это работает.',
      },
      features: [
        {
          shot: 'editor',
          en: {
            title: 'An editor with language servers that are actually there',
            body: 'CodeMirror 6 over an LSP client, with language servers included: a packaged cide carries its own rust-analyzer — a fork with a disk index, so a big workspace is warm on the second launch — and its own gopls. Extensions can add any other server.',
            points: [
              'Completion with auto-imports resolved on demand',
              'Go to definition and implementation, Find usages, quick docs',
              'IDEA-style change bars in the gutter, blame and folding',
              'Shift+Alt+F reformats, and diagnostics follow the disk',
            ],
          },
          ru: {
            title: 'Редактор с языковыми серверами, которые действительно есть',
            body: 'CodeMirror 6 поверх LSP-клиента, и серверы идут в комплекте: собранный cide содержит свой rust-analyzer — форк с дисковым индексом, так что большой проект открывается «прогретым» уже со второго запуска, — и свой gopls. Любой другой сервер можно добавить расширением.',
            points: [
              'Автодополнение с автоимпортом, который подгружается по запросу',
              'Переход к определению и реализации, поиск использований, быстрая документация',
              'Метки изменений на полях в стиле IDEA, blame и сворачивание',
              'Shift+Alt+F форматирует код, а диагностика следит за диском',
            ],
          },
        },
        {
          shot: 'search',
          en: {
            title: 'Find anything',
            body: 'Ctrl+P for files, Ctrl+Shift+F for text, Ctrl+Shift+P for every command. Search across the project, narrow it to a folder or a file pattern, and type to filter any tree in the sidebar.',
            points: [
              'Content search, with scope and file pattern',
              'A command palette over the same registry as the keymap',
              'A find bar in terminal panes',
            ],
          },
          ru: {
            title: 'Найдётся всё',
            body: 'Ctrl+P — файлы, Ctrl+Shift+F — текст, Ctrl+Shift+P — любая команда. Ищите по всему проекту, сужайте до папки или маски файлов, а в боковой панели просто начните печатать.',
            points: [
              'Поиск по содержимому с областью и маской файлов',
              'Палитра команд на том же реестре, что и горячие клавиши',
              'Поиск по тексту в терминальных панелях',
            ],
          },
        },
        {
          shot: 'drawing',
          en: {
            title: 'Docs and diagrams next to the code',
            body: 'A markdown file opens beside its rendered preview, and the two scroll together. An Excalidraw drawing opens as a drawing, in a full Excalidraw editor inside the tab, and is saved back to the file it came from.',
            points: [
              'Preview with highlighted code fences, tables and local images',
              'Edit .excalidraw, .excalidraw.json, .excalidraw.svg and .excalidraw.png files',
              'A drawing kept as .svg or .png still shows as a picture everywhere else',
            ],
          },
          ru: {
            title: 'Документы и схемы рядом с кодом',
            body: 'Markdown-файл открывается рядом со своим предпросмотром, и они прокручиваются вместе. Рисунок Excalidraw открывается как рисунок — в полноценном редакторе Excalidraw прямо во вкладке — и сохраняется обратно в свой файл.',
            points: [
              'Предпросмотр с подсветкой блоков кода, таблицами и локальными картинками',
              'Редактирование файлов .excalidraw, .excalidraw.json, .excalidraw.svg и .excalidraw.png',
              'Рисунок в .svg или .png остаётся обычной картинкой везде за пределами cide',
            ],
          },
        },
        {
          shot: 'git',
          en: {
            title: 'Git the way IDEA does it',
            body: 'Several repositories in one project, changelists instead of the index, a shelf, and staging down to single hunks and lines. The commit tool window docks at the bottom, where an IDEA user looks for it.',
            points: [
              'Changelists and a shelf for unrelated edits',
              'Stage a hunk or a single line',
              'Several repositories and submodules in one tree',
            ],
          },
          ru: {
            title: 'Git, как в IDEA',
            body: 'Несколько репозиториев в одном проекте, changelists вместо индекса, полка и индексирование вплоть до отдельных блоков и строк. Окно коммита пристыковано снизу — там, где его ищет пользователь IDEA.',
            points: [
              'Changelists и полка для несвязанных правок',
              'Индексирование отдельного блока или одной строки',
              'Несколько репозиториев и подмодулей в одном дереве',
            ],
          },
        },
        {
          shot: 'log',
          en: {
            title: 'A log with graph lanes',
            body: 'The history as a graph, with file history, blame and the commit actions you use every day: amend, reset, tag, cherry-pick and revert. Push shows what will leave first, and uses force-with-lease when you force.',
            points: [
              'Graph lanes, branch and tag chips, search by hash or text',
              'Amend, reset, cherry-pick, revert, tag',
              'A push preview and force-with-lease',
            ],
          },
          ru: {
            title: 'Лог с графом веток',
            body: 'История в виде графа, история файла, blame и повседневные действия с коммитами: amend, reset, тег, cherry-pick и revert. Push сначала показывает, что уйдёт, а при принудительной отправке использует force-with-lease.',
            points: [
              'Граф веток, метки веток и тегов, поиск по хешу или тексту',
              'Amend, reset, cherry-pick, revert, теги',
              'Предпросмотр push и force-with-lease',
            ],
          },
        },
        {
          shot: 'merge',
          en: {
            title: 'Conflicts, resolved in three panes',
            body: 'Pull with merge or rebase, and when it stops, resolve each file in a three-pane merge driven by git’s actual in-progress operation state, not a guess about it.',
            points: [
              'Yours, the result and theirs, side by side',
              'Accept a side per chunk, or edit the result directly',
              'Continue or abort the merge or rebase from the IDE',
            ],
          },
          ru: {
            title: 'Конфликты — в трёх панелях',
            body: 'Pull через merge или rebase, а если он остановился — разрешайте каждый файл в трёхпанельном слиянии, опирающемся на реальное состояние незавершённой операции git, а не на догадки.',
            points: [
              'Ваша версия, результат и их версия рядом',
              'Выбор стороны для каждого блока или правка результата вручную',
              'Продолжение или отмена merge и rebase прямо из IDE',
            ],
          },
        },
      ],
    },
    {
      id: 'workflow',
      en: {
        title: 'Your whole workflow',
        lead: 'Code review, specs and containers, in the same window as the session.',
      },
      ru: {
        title: 'Весь рабочий процесс',
        lead: 'Ревью кода, спецификации и контейнеры — в том же окне, что и сессия.',
      },
      features: [
        {
          shot: 'gitlab',
          en: {
            title: 'Review merge requests without leaving',
            body: 'A GitLab inbox for what you should review and what you opened, inline threads on a hunks-only diff, approvals, and pipelines with their job logs in colour. Review with an agent and it drafts severity-tagged comments, which you publish yourself.',
            points: [
              'Inbox: to review, created, assigned',
              'Inline threads, approvals and pipelines',
              'Agent review writes drafts, never posts',
              'Read-only source checkouts with go-to-definition',
            ],
          },
          ru: {
            title: 'Ревью merge request, не выходя из IDE',
            body: 'Входящие GitLab — что вам ревьюить и что вы открыли, обсуждения прямо в диффе, одобрения и пайплайны с цветными логами задач. Попросите агента провести ревью — он подготовит черновики комментариев с пометкой важности, а публикуете их вы.',
            points: [
              'Входящие: на ревью, созданные, назначенные',
              'Обсуждения в коде, одобрения и пайплайны',
              'Агент пишет черновики и никогда не публикует сам',
              'Исходники только для чтения с переходом к определению',
            ],
          },
        },
        {
          shot: 'openspec',
          en: {
            title: 'Specs your agents work from',
            body: 'If a project uses OpenSpec, cide shows its changes, specs, requirement deltas and checklists as a panel and as documents, by running the openspec CLI rather than parsing its files. A change becomes a task in one step, and workflow commands go straight into the running session.',
            points: [
              'Changes, specs, deltas and checklists',
              'Open a change as a task',
              'The OpenSpec workflow, run in the session you already have',
            ],
          },
          ru: {
            title: 'Спецификации, по которым работают агенты',
            body: 'Если в проекте есть OpenSpec, cide показывает изменения, спецификации, дельты требований и чек-листы — панелью и документами. Данные берутся из CLI openspec, а не разбором файлов. Изменение превращается в задачу одним действием, а команды процесса уходят прямо в запущенную сессию.',
            points: [
              'Изменения, спецификации, дельты и чек-листы',
              'Изменение можно открыть как задачу',
              'Процесс OpenSpec — в уже открытой сессии',
            ],
          },
        },
        {
          shot: 'docker',
          en: {
            title: 'Containers next to your code',
            body: 'Containers, images, volumes and networks, with inspect and remove. Exec and log panes are real terminals in the grid, a container’s files open like any other tree, and a compose file starts from a mark in the gutter.',
            points: [
              'Exec and log panes as real terminals',
              'Browse a container’s filesystem',
              'Compose up from the editor, and Dockerfile language support',
            ],
          },
          ru: {
            title: 'Контейнеры рядом с кодом',
            body: 'Контейнеры, образы, тома и сети — с просмотром и удалением. Панели exec и логов — настоящие терминалы в сетке, файлы контейнера открываются как обычное дерево, а compose запускается по метке на полях редактора.',
            points: [
              'Exec и логи — настоящие терминалы',
              'Просмотр файловой системы контейнера',
              'Compose up из редактора и поддержка Dockerfile',
            ],
          },
        },
      ],
    },
    {
      id: 'yours',
      en: {
        title: 'Yours to shape',
        lead: 'Themes, keys and extensions, with nothing hidden behind a JSON file.',
      },
      ru: {
        title: 'Настройте под себя',
        lead: 'Темы, клавиши и расширения — без правки JSON вслепую.',
      },
      features: [
        {
          shot: 'settings-scheme',
          en: {
            title: 'Bring your VS Code theme',
            body: 'Dark and light out of the box, and any VS Code colour theme imported from a .vsix or a bare -color-theme.json and converted to cide’s own token roles, so the editor, the terminal and the chrome all agree.',
            points: [
              'Separate schemes for light and dark',
              'One import colours the editor, the terminals and the diff',
              'Interface scale and font sizes',
            ],
          },
          ru: {
            title: 'Ваша тема из VS Code',
            body: 'Тёмная и светлая темы из коробки, плюс любая цветовая тема VS Code, импортированная из .vsix или отдельного -color-theme.json и переведённая в роли токенов cide — так что редактор, терминал и интерфейс выглядят согласованно.',
            points: [
              'Отдельные схемы для светлой и тёмной темы',
              'Один импорт раскрашивает редактор, терминалы и диффы',
              'Масштаб интерфейса и размеры шрифтов',
            ],
          },
        },
        {
          shot: 'keymap',
          en: {
            title: 'One registry for every key',
            body: 'The keymap editor, the command palette and the bindings all read from one command registry, so a command you can find is a command you can bind. Shortcuts keep working under non-Latin keyboard layouts.',
            points: [
              'Rebind anything, and see conflicts as you type',
              'Chords and per-context bindings',
              'Works in any keyboard layout',
            ],
          },
          ru: {
            title: 'Один реестр для всех клавиш',
            body: 'Редактор горячих клавиш, палитра команд и сами привязки читают один реестр команд: любую команду, которую можно найти, можно и назначить на клавишу. Сочетания работают и в нелатинских раскладках.',
            points: [
              'Переназначайте что угодно и сразу видите конфликты',
              'Аккорды и привязки для разных контекстов',
              'Работает в любой раскладке',
            ],
          },
        },
        {
          shot: 'extensions',
          en: {
            title: 'Extensions from a git repository',
            body: 'A marketplace is just a git repository. Extensions add language servers, panels, settings and commands — Godot with its language server, or a SQL workbench — and cide shows which contribution wins when two collide.',
            points: [
              'Connect a marketplace by URL',
              'Language servers, panels, settings and commands',
              'Clear conflict reporting',
            ],
          },
          ru: {
            title: 'Расширения из git-репозитория',
            body: 'Маркетплейс — это просто git-репозиторий. Расширения добавляют языковые серверы, панели, настройки и команды — например, Godot с его языковым сервером или SQL-консоль, — а если два расширения конфликтуют, cide показывает, какое победило.',
            points: [
              'Маркетплейс подключается по URL',
              'Языковые серверы, панели, настройки и команды',
              'Понятные отчёты о конфликтах',
            ],
          },
        },
        {
          shot: 'new-project',
          en: {
            title: 'Start a project that agents can pick up',
            body: 'The New Project wizard offers three starts: an empty project, one driven by OpenSpec, or one driven by tasks written from a short brief. Agents can pick the work up from the first minute.',
            points: [
              'Empty, OpenSpec-driven, or task-driven from a brief',
              'Roles and a tracker set up for you',
              'Opens straight onto its Claude tab',
            ],
          },
          ru: {
            title: 'Проект, готовый для агентов с первой минуты',
            body: 'Мастер нового проекта предлагает три варианта: пустой проект, проект на OpenSpec или проект с задачами, составленными по короткому описанию. Агенты могут взяться за работу сразу.',
            points: [
              'Пустой, на OpenSpec или из задач по описанию',
              'Роли и трекер настраиваются за вас',
              'Проект сразу открывается на вкладке Claude',
            ],
          },
        },
      ],
    },
  ],
}
