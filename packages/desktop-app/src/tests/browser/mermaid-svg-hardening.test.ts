/* global CSSStyleSheet, CSSStyleRule */
import { describe, it, expect, vi, afterEach } from 'vitest';
import mermaid from 'mermaid';
import { sanitizeSvg, renderMermaid } from '../../lib/services/mermaid-render.js';
import { isScopedSelector } from '../../lib/services/svg-css-sanitize.js';

const parse = (svg: string) => new window.DOMParser().parseFromString(svg, 'image/svg+xml').documentElement;
const styleText = (root: Element) =>
  Array.from(root.querySelectorAll('style'))
    .map((s) => s.textContent ?? '')
    .join('\n');
const shapes = (root: Element) => ({
  text: root.querySelectorAll('text, tspan').length,
  paths: root.querySelectorAll('path, rect, circle, polygon, line, ellipse').length,
  markers: root.querySelectorAll('marker').length
});
const sheetOf = (css: string) => {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(css);
  return Array.from(sheet.cssRules);
};

const wrap = (inner: string) => `<svg id="d1" viewBox="0 0 10 10">${inner}</svg>`;
const sanitizeStyle = (css: string) => styleText(parse(sanitizeSvg(wrap(`<style>${css}</style><rect/>`))));

/** Every surviving rule is a top-level style rule under #d1 with safe declarations. */
function expectAllowListed(css: string) {
  for (const rule of sheetOf(css)) {
    expect(rule.type, rule.cssText).toBe(1);
    const style = rule as CSSStyleRule;
    expect(style.cssRules?.length ?? 0).toBe(0);
    for (const sel of style.selectorText.split(',')) {
      expect(isScopedSelector(sel.trim(), 'd1'), sel).toBe(true);
    }
    expect(rule.cssText).not.toMatch(/@|var\(|\\|(^|[\s{;])(position|inset|z-index|top|left)\s*:/i);
    const nonFragment = rule.cssText.replace(/url\(\s*["']?#[\w.:-]+["']?\s*\)/gi, '');
    expect(nonFragment).not.toMatch(/url\(/i);
  }
}

const HOSTILE_STYLES: Record<string, string> = {
  'brace in string': '#d1{a:"{"} body{display:none} }',
  'brace in url': '#d1{background:url(#{)} p{color:red} }',
  'brace in string in media': '@media screen{#d1{a:"{"} body{display:none} }}',
  import: '@import url(https://evil.test/x.css); #d1 .a{fill:red}',
  scope: '@scope (body){*{display:none}}',
  container: '@container (min-width:1px){body{display:none}}',
  'starting-style': '@starting-style{body{opacity:0}}',
  keyframes: '@keyframes spin{to{transform:rotate(1turn)}} #d1 .a{fill:red}',
  property: '@property --x{syntax:"<length>";inherits:false;initial-value:0px}',
  page: '@page{margin:0}',
  'font-face': '@font-face{font-family:x;src:url(https://evil.test/f.woff)}',
  'nested at-rule': '@media screen{@supports (display:grid){body{display:none}}}',
  'nesting not': '#d1{ :not(&){display:none} }',
  'nesting root has': '#d1{ :root:has(&){filter:blur(9px)} }',
  'nesting sibling': '#d1{ & ~ *{display:none} }',
  'nesting media': '#d1{ @media screen { body{display:none} } }',
  'general sibling': '#d1 ~ *{display:none}',
  'adjacent sibling': '#d1 + div{display:none}',
  has: '#d1:has(~ *){display:none}',
  'id elsewhere': 'body #d1{display:none}',
  'id prefix only': '#d1x{display:none}',
  'unscoped svg': 'svg{position:fixed;top:0;left:0;width:100vw;height:100vh}',
  'var position': '#d1{--p:fixed;position:var(--p)}',
  'fixed position': '#d1 .a{position:fixed;inset:0;z-index:99999;pointer-events:none}',
  'remote url': '#d1 .a{fill:url(https://evil.test/p);background:image-set("https://evil.test/i.png" 1x)}',
  'escaped url': '#d1 .a{fill:u\\72l(https://evil.test/p)}',
  'close style tag': '#d1 .a{content:"</style><img src=x onerror=alert(1)>"}'
};

describe('hostile <style> text is rebuilt from the allow-list', () => {
  for (const [name, css] of Object.entries(HOSTILE_STYLES)) {
    it(name, () => {
      const out = sanitizeStyle(css);
      expectAllowListed(out);
      expect(out).not.toContain('evil.test');
      expect(out).not.toMatch(/body|display:\s*none|@/);
    });
  }

  it('keeps legitimate scoped rules, fragment urls and pseudo-classes', () => {
    const out = sanitizeStyle(
      '#d1 .node rect, #d1 > g:first-child{fill:#fff;stroke:#333333 !important}#d1 .m{marker-end:url(#arrow)}#d1 a:hover text{fill:red}'
    );
    expect(out).toContain('#d1 .node rect');
    expect(out).toContain('#d1 > g:first-child');
    expect(out).toContain('url("#arrow")');
    expect(out).toContain('#d1 a:hover text');
    expect(out).toContain('!important');
  });

  it('restricts rules that style the diagram root itself', () => {
    const out = sanitizeStyle(
      '#d1{transform:scale(50);translate:9px;scale:9;rotate:9deg;offset-path:none;margin-top:-999px;float:left;' +
        'overflow:visible;overflow-x:visible;width:100vw;height:100vh;max-width:9px;clip-path:circle(1px);opacity:.1;' +
        'mix-blend-mode:difference;filter:blur(9px);backdrop-filter:blur(9px);display:block;contain:none;' +
        'isolation:auto;will-change:transform;perspective:1px;font-family:serif;color:#112233}' +
        '#d1:hover{transform:scale(50);fill:red}' +
        '#d1 .a{opacity:.5;transform:rotate(3deg);margin:0}'
    );
    expect(out).toContain('font-family');
    expect(out).toContain('color');
    expect(out).toContain('fill:red');
    expect(out).toMatch(/#d1 \.a\{[^}]*transform:\s*rotate\(3deg\)/);
    const rootRules = sheetOf(out).filter((r) => !/#d1 \./.test((r as CSSStyleRule).selectorText));
    expect(rootRules.length).toBe(2);
    for (const rule of rootRules) {
      expect(rule.cssText).not.toMatch(
        /transform|translate|scale\s*:|rotate\s*:|offset|margin|float|overflow|(?<!max-)width|height|clip|opacity|blend|filter|display|contain|isolation|will-change|perspective/i
      );
    }
  });

  it('drops attr() values and animation/transition properties', () => {
    const out = sanitizeStyle(
      '#d1 .a{background:url(attr(data-x));fill:attr(data-y);animation:spin 1s infinite;animation-name:ping;' +
        'transition:all 1s;transition-property:all;stroke:blue}'
    );
    expect(out).not.toMatch(/attr|animation|transition|spin|ping/i);
    expect(out).toContain('stroke');
  });

  it('forces containment on the root svg, whatever its own style says', () => {
    const out = sanitizeSvg(
      '<svg id="d1" style="transform:scale(40);overflow:visible !important;max-width:100px"><rect/></svg>'
    );
    const root = parse(out);
    const style = root.getAttribute('style') ?? '';
    expect(style).toContain('max-width');
    expect(style).not.toContain('transform');
    expect(style).not.toMatch(/overflow:\s*visible/);
    expect(style).toMatch(/overflow:\s*hidden\s*!important/);
    expect(style).toContain('contain:paint');
    expect(style).toContain('isolation:isolate');
    expect(sanitizeSvg('<svg id="d1"><rect/></svg>')).toMatch(/overflow:\s*hidden/);
  });

  it('drops the style element when the environment has no CSS parser', () => {
    vi.stubGlobal('CSSStyleSheet', undefined);
    try {
      const out = sanitizeSvg(wrap('<style>#d1 .a{fill:red}</style><rect class="a"/>'));
      expect(out).not.toContain('<style');
      expect(out).toContain('<rect');
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('drops a style element with no root id to scope it', () => {
    expect(sanitizeSvg('<svg><style>.a{fill:red}</style><rect/></svg>')).not.toContain('<style');
  });
});

describe('attributes', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('drops remote url(), including backslash-escaped and commented forms', () => {
    const out = sanitizeSvg(
      wrap(
        '<rect fill="url(https://evil.test/p)" filter="url(\'https://evil.test/f\')" mask="u\\rl(https://evil.test/m)"' +
          ' clip-path="u\\72l(https://evil.test/c)" stroke="ur/**/l(https://evil.test/s)" cursor="url(//evil.test/c.cur)"' +
          ' marker-end="url(#arrow)" marker-start="url(&quot;#arrow&quot;)"/>'
      )
    );
    expect(out).not.toContain('evil.test');
    expect(out).toContain('marker-end="url(#arrow)"');
  });

  it('re-serialises the style attribute and drops overlay and remote constructs', () => {
    const out = sanitizeSvg(
      wrap(
        '<rect style="fill:url(https://evil.test/s);position:fixed;top:0;--p:fixed;position:var(--p);stroke:#333333;z-index:9"/>' +
          '<rect style="fill:u\\72l(https://evil.test/e)"/><rect style="background:url(#a)"/>'
      )
    );
    expect(out).not.toContain('evil.test');
    expect(out).not.toMatch(/(^|[\s"';])(position|z-index|top)\s*:|var\(/);
    expect(out).toContain('stroke');
  });

  it('keeps only fragment or data-image hrefs on image and feImage', () => {
    const out = sanitizeSvg(
      wrap(
        '<image href="https://evil.test/i.png"/><image xlink:href="//evil.test/j.png"/>' +
          '<filter><feImage href="https://evil.test/k.png"/></filter><use href="https://evil.test/u.svg#a"/>' +
          '<image href="#local"/><image href="data:image/png;base64,AAAA"/>'
      )
    );
    expect(out).not.toContain('evil.test');
    expect(out).toContain('href="#local"');
    expect(out).toContain('data:image/png');
  });
});

const DIAGRAMS: Record<string, string> = {
  flowchart: 'graph TD; A[Start]-->B{Ok?}; B-->|yes|C((End)); B-->|no|A',
  sequence: 'sequenceDiagram\nA->>B: hi\nB-->>A: yo',
  class: 'classDiagram\nA <|-- B\nA : +int x',
  state: 'stateDiagram-v2\n[*] --> S1\nS1 --> [*]',
  gantt: 'gantt\ndateFormat YYYY-MM-DD\nsection S\nTask one :a1, 2024-01-01, 5d\nTask two :after a1, 3d',
  pie: 'pie title Pets\n"Dogs" : 5\n"Cats" : 3',
  er: 'erDiagram\nCUSTOMER ||--o{ ORDER : places',
  mindmap: 'mindmap\n  root((Root))\n    Child A\n    Child B',
  gitGraph: 'gitGraph\ncommit\nbranch dev\ncommit\ncheckout main\nmerge dev'
};

// Top-level style rules and at-rules the allow-list drops, per diagram type.
// Every diagram loses its `:root` rule (only a font custom property) and its
// two @keyframes (edge animation). Sequence also loses `.actorPopupMenu`
// (positioning only) and mindmap has six empty rules.
const EXPECTED_LOSS: Record<string, { rules: number; atRules: number }> = {
  flowchart: { rules: 1, atRules: 2 },
  sequence: { rules: 2, atRules: 2 },
  class: { rules: 1, atRules: 2 },
  state: { rules: 1, atRules: 2 },
  gantt: { rules: 1, atRules: 2 },
  pie: { rules: 1, atRules: 2 },
  er: { rules: 1, atRules: 2 },
  mindmap: { rules: 7, atRules: 2 },
  gitGraph: { rules: 1, atRules: 2 }
};

describe('real mermaid output through sanitizeSvg', () => {
  const report: Record<string, { rules: number; atRules: number }> = {};
  for (const [name, def] of Object.entries(DIAGRAMS)) {
    it(`keeps shapes, text, markers and scoped style: ${name}`, async () => {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: 'base',
        htmlLabels: false,
        flowchart: { htmlLabels: false },
        class: { htmlLabels: false },
        themeVariables: { primaryColor: '#123456', lineColor: '#654321' }
      });
      const id = `real-${name}`;
      const { svg: raw } = await mermaid.render(id, def);
      const rawRoot = parse(raw);
      const out = sanitizeSvg(raw);
      const outRoot = parse(out);
      expect(shapes(outRoot)).toEqual(shapes(rawRoot));
      expect(shapes(outRoot).text).toBeGreaterThan(0);

      const rawRules = sheetOf(styleText(rawRoot));
      const outRules = sheetOf(styleText(outRoot));
      expect(outRules.length).toBeGreaterThan(0);
      for (const rule of outRules) {
        expect(rule.type).toBe(1);
        for (const sel of (rule as CSSStyleRule).selectorText.split(',')) {
          expect(sel.trim().startsWith(`#${id}`), sel).toBe(true);
        }
      }
      const rawStyleRules = rawRules.filter((r) => r.type === 1).length;
      report[name] = { rules: rawStyleRules - outRules.length, atRules: rawRules.length - rawStyleRules };
      expect(report[name]).toEqual(EXPECTED_LOSS[name]);
    });
  }

  it('a themeCSS payload cannot paint outside the diagram box', async () => {
    const def =
      '%%{init: {"themeCSS": "&{transform:scale(40);transform-origin:0 0;overflow:visible;margin-top:-999px} ' +
      '.node rect{fill:#c00;stroke:none}"}}%%\ngraph TD; A[Click here to claim]';
    const out = (await renderMermaid(def, 'themecss', false)) ?? '';
    expect(out).toContain('<rect');
    const host = document.createElement('div');
    host.style.cssText = 'position:absolute;left:300px;top:300px;width:200px;height:120px';
    host.innerHTML = out;
    const target = document.createElement('div');
    target.style.cssText = 'position:absolute;left:0;top:0;width:100vw;height:100vh';
    document.body.append(target, host);
    try {
      const root = host.querySelector('svg') as Element;
      const computed = getComputedStyle(root);
      expect(computed.overflow).toBe('hidden');
      expect(computed.transform).toBe('none');
      expect(computed.marginTop).toBe('0px');
      const box = host.getBoundingClientRect();
      for (const [x, y] of [
        [box.left - 100, box.top - 100],
        [box.right + 100, box.bottom + 100],
        [box.left - 100, box.bottom + 100]
      ]) {
        const hit = document.elementFromPoint(x, y);
        expect(host.contains(hit), `${x},${y}`).toBe(false);
      }
    } finally {
      host.remove();
      target.remove();
    }
  });

  it('renderMermaid keeps a themed dark flowchart styled', async () => {
    const out = (await renderMermaid(DIAGRAMS.flowchart, 'dark', true)) ?? '';
    const root = parse(out);
    expect(shapes(root).text).toBeGreaterThan(0);
    expect(styleText(root)).toMatch(/#mermaid-dark-\d+/);
  });
});
