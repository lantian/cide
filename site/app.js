/*
 * The site's behaviour: render the feature chapters, switch language, switch theme, zoom a shot.
 *
 * The theme switch is the product's own: cide ships a dark and a light theme, and every
 * screenshot exists in both (`img/<shot>-dark.webp`, `img/<shot>-light.webp`). So switching the
 * theme here also swaps every picture to the matching render of the app, not only the page colours
 * around it.
 */
;(function () {
  var site = window.CIDE_SITE
  var root = document.documentElement

  function store(key, value) {
    try {
      localStorage.setItem(key, value)
    } catch (e) {
      // A private window or blocked storage: the choice lasts until the tab closes, which is fine.
    }
  }

  function t(key) {
    var lang = root.lang
    return (site.ui[lang] && site.ui[lang][key]) || site.ui.en[key] || key
  }

  function el(tag, cls, text) {
    var node = document.createElement(tag)
    if (cls) node.className = cls
    if (text != null) node.textContent = text
    return node
  }

  function shotSrc(shot) {
    return 'img/' + shot + '-' + root.dataset.theme + '.webp'
  }

  // --- chapters ------------------------------------------------------------------------

  function renderFeatures() {
    var lang = root.lang
    var host = document.getElementById('features')
    host.textContent = ''
    site.chapters.forEach(function (chapter, ci) {
      var copy = chapter[lang] || chapter.en
      var section = el('section', 'chapter')
      section.id = chapter.id
      var head = el('header', 'chapter-head')
      head.appendChild(el('span', 'chapter-num', String(ci + 1).padStart(2, '0')))
      head.appendChild(el('h2', null, copy.title))
      head.appendChild(el('p', null, copy.lead))
      section.appendChild(head)
      chapter.features.forEach(function (feature) {
        var f = feature[lang] || feature.en
        var row = el('article', 'feature')
        var text = el('div', 'feature-text')
        text.appendChild(el('h3', null, f.title))
        text.appendChild(el('p', null, f.body))
        var list = el('ul')
        f.points.forEach(function (p) {
          list.appendChild(el('li', null, p))
        })
        text.appendChild(list)
        var fig = el('figure', 'feature-shot')
        var frame = el('button', 'frame')
        frame.type = 'button'
        frame.setAttribute('data-zoom', '')
        var img = el('img')
        img.dataset.shot = feature.shot
        img.src = shotSrc(feature.shot)
        img.alt = f.title
        img.loading = 'lazy'
        img.decoding = 'async'
        img.width = 2880
        img.height = 1800
        frame.appendChild(img)
        fig.appendChild(frame)
        row.appendChild(text)
        row.appendChild(fig)
        section.appendChild(row)
      })
      host.appendChild(section)
    })
    observe()
  }

  // --- language ------------------------------------------------------------------------

  function applyLang(lang) {
    root.lang = lang
    document.querySelectorAll('[data-i18n]').forEach(function (node) {
      node.textContent = t(node.getAttribute('data-i18n'))
    })
    document.querySelectorAll('[data-i18n-label]').forEach(function (node) {
      node.setAttribute('aria-label', t(node.getAttribute('data-i18n-label')))
    })
    document.querySelectorAll('.lang button').forEach(function (b) {
      b.setAttribute('aria-pressed', String(b.dataset.lang === lang))
    })
    document.title = lang === 'ru' ? 'cide — IDE, построенная вокруг Claude Code' : 'cide — the IDE built around Claude Code'
    renderFeatures()
    applyThemeLabel()
  }

  // --- theme ---------------------------------------------------------------------------

  function applyThemeLabel() {
    var button = document.getElementById('theme')
    button.setAttribute('aria-label', t(root.dataset.theme === 'dark' ? 'theme.toLight' : 'theme.toDark'))
  }

  function applyTheme(theme) {
    root.dataset.theme = theme
    document.querySelectorAll('img[data-shot]').forEach(function (img) {
      img.src = shotSrc(img.dataset.shot)
    })
    var open = document.querySelector('#lightbox img')
    if (open && open.dataset.shot) open.src = shotSrc(open.dataset.shot)
    applyThemeLabel()
  }

  // --- reveal on scroll ----------------------------------------------------------------

  var io =
    'IntersectionObserver' in window && !matchMedia('(prefers-reduced-motion: reduce)').matches
      ? new IntersectionObserver(
          function (entries) {
            entries.forEach(function (e) {
              if (e.isIntersecting) {
                e.target.classList.add('in')
                io.unobserve(e.target)
              }
            })
          },
          { rootMargin: '0px 0px -10% 0px' },
        )
      : null

  function observe() {
    document.querySelectorAll('.feature, .pillars article, .chapter-head').forEach(function (node) {
      if (io) {
        node.classList.add('reveal')
        io.observe(node)
      }
    })
  }

  // --- lightbox ------------------------------------------------------------------------

  var box = document.getElementById('lightbox')
  document.addEventListener('click', function (e) {
    var frame = e.target.closest && e.target.closest('[data-zoom]')
    if (!frame) return
    var img = frame.querySelector('img')
    var big = box.querySelector('img')
    big.dataset.shot = img.dataset.shot
    big.src = shotSrc(img.dataset.shot)
    big.alt = img.alt
    if (typeof box.showModal === 'function') box.showModal()
  })
  box.addEventListener('click', function (e) {
    if (e.target === box || e.target.classList.contains('close')) box.close()
  })

  // --- wiring --------------------------------------------------------------------------

  document.querySelectorAll('.lang button').forEach(function (b) {
    b.addEventListener('click', function () {
      store('cide-site-lang', b.dataset.lang)
      applyLang(b.dataset.lang)
    })
  })
  document.getElementById('theme').addEventListener('click', function () {
    var next = root.dataset.theme === 'dark' ? 'light' : 'dark'
    store('cide-site-theme', next)
    applyTheme(next)
  })
  document.getElementById('copy').addEventListener('click', function () {
    var button = this
    var text = document.getElementById('install-code').textContent
    var done = function () {
      button.textContent = t('install.copied')
      setTimeout(function () {
        button.textContent = t('install.copy')
      }, 1600)
    }
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(done, function () {})
  })

  applyLang(root.lang === 'ru' ? 'ru' : 'en')
  applyTheme(root.dataset.theme === 'light' ? 'light' : 'dark')
})()
