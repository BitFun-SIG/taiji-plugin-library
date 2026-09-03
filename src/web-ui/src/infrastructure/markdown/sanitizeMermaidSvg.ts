/**
 * Sanitizer for Mermaid-rendered SVG markup, kept in its own module so the
 * MermaidBlock component file only exports components (react-refresh /
 * `only-export-components` fast-refresh contract).
 */

/**
 * Sanitize Mermaid-rendered SVG markup before it is injected via
 * dangerouslySetInnerHTML.
 *
 * Mermaid is initialized with `securityLevel: 'loose'`, which lets label markup
 * such as `<img onerror=...>`, `<script>`, `on*` event attributes and
 * `javascript:` URLs survive rendering. We strip those while keeping the
 * structural SVG and text-container elements a rendered diagram needs.
 */
const ALLOWED_SVG_TAGS = new Set([
  // svg structure / shapes
  'svg', 'g', 'defs', 'style', 'marker', 'path', 'rect', 'circle', 'ellipse',
  'line', 'polyline', 'polygon', 'text', 'tspan', 'title', 'desc', 'use',
  'symbol', 'textpath', 'clippath', 'stop', 'pattern', 'mask', 'filter',
  'fegaussianblur', 'fecolormatrix', 'feoffset', 'feflood', 'feblend',
  'image', 'a',
  // gradient / pattern
  'lineargradient', 'radialgradient',
  // foreignObject content (mermaid htmlLabels) + text containers
  'foreignobject', 'div', 'span', 'p', 'br', 'b', 'strong', 'i', 'em', 'u',
  'small', 'sub', 'sup', 'ul', 'ol', 'li', 'pre', 'code', 'font',
  'table', 'thead', 'tbody', 'tfoot', 'tr', 'td', 'th', 'figure', 'h1', 'h2',
  'h3', 'h4', 'h5', 'h6',
]);

const ALLOWED_SVG_ATTRS = new Set([
  // core + namespace
  'id', 'class', 'xmlns', 'xmlns:xlink', 'xlink:href', 'xlink:title',
  'xml:space', 'role', 'aria-label', 'aria-roledescription', 'aria-hidden',
  'focusable', 'tabindex', 'title',
  // geometry / viewport
  'viewbox', 'preserveaspectratio', 'width', 'height', 'x', 'y', 'x1', 'y1',
  'x2', 'y2', 'cx', 'cy', 'r', 'rx', 'ry', 'd', 'points', 'pathlength',
  'transform', 'translate', 'scale', 'rotate', 'skewx', 'skewy', 'matrix',
  'offset', 'refx', 'refy', 'refwidth', 'refheight',
  // painting
  'fill', 'fill-opacity', 'fill-rule', 'stroke', 'stroke-width',
  'stroke-linecap', 'stroke-linejoin', 'stroke-miterlimit', 'stroke-dasharray',
  'stroke-dashoffset', 'stroke-opacity', 'opacity', 'color', 'flood-color',
  'flood-opacity', 'stop-color', 'stop-opacity',
  // typography / text
  'font-family', 'font-size', 'font-weight', 'font-style', 'font-variant',
  'text-anchor', 'dominant-baseline', 'baseline-shift', 'letter-spacing',
  'word-spacing', 'text-decoration', 'direction', 'unicode-bidi',
  'writing-mode', 'alignment-baseline', 'line-height', 'text-transform',
  'white-space', 'font-stretch',
  // markers / gradients / patterns / filters
  'marker-start', 'marker-mid', 'marker-end', 'marker', 'markerwidth',
  'markerheight', 'markerunits', 'orient', 'gradientunits', 'gradienttransform',
  'spreadmethod', 'patternunits', 'patterncontentunits', 'patterntransform',
  'maskunits', 'maskcontentunits', 'in', 'in2', 'result', 'stddeviation',
  'edgemode', 'kernelunitlength', 'tablevalues', 'values', 'type', 'mode',
  'interpolate', 'numoctaves', 'basefrequency', 'stitchtiles',
  // styles / display
  'style', 'display', 'visibility', 'overflow', 'clip', 'clip-path', 'clip-rule',
  'vector-effect', 'shape-rendering', 'text-rendering', 'image-rendering',
  'color-rendering', 'color-interpolation', 'color-interpolation-filters',
  'paint-order', 'mix-blend-mode', 'isolation', 'pointer-events', 'cursor',
  'filter', 'mask', 'enable-background',
  // animation
  'attributename', 'attributetype', 'begin', 'dur', 'end', 'repeatcount',
  'repeatdur', 'from', 'to', 'by', 'calcmode', 'additive', 'keytimes',
  'keysplines', 'keypoints', 'restart', 'fill-freeze',
  // html container attributes
  'align', 'valign', 'colspan', 'rowspan', 'cellpadding', 'cellspacing',
  'border', 'bgcolor', 'nowrap', 'dir', 'lang', 'col', 'row', 'span',
  'start', 'reversed', 'data-label',
]);

const DANGEROUS_TAGS = new Set([
  'script', 'iframe', 'object', 'embed', 'form', 'meta', 'link', 'base',
  'frame', 'frameset', 'noscript', 'template',
]);

const EVENT_ATTR_RE = /^on/i;
const URL_ATTRS = new Set(['href', 'src', 'xlink:href']);
const UNSAFE_URL_RE = /^\s*(?:javascript|vbscript|data:text\/html)\s*:/i;

export function sanitizeMermaidSvg(svgRaw: string): string {
  if (!svgRaw) return svgRaw;

  let doc: Document;
  try {
    doc = new DOMParser().parseFromString(svgRaw, 'text/html');
  } catch {
    return '';
  }

  const root = doc.body.firstElementChild;
  if (!root || root.localName.toLowerCase() !== 'svg') return '';

  const sanitizeNode = (node: Element) => {
    const tag = node.localName.toLowerCase();
    // Any element outside the allowed set (scripts, iframes, unknown HTML
    // injected via labels, etc.) is removed together with its subtree.
    if (DANGEROUS_TAGS.has(tag) || !ALLOWED_SVG_TAGS.has(tag)) {
      node.remove();
      return;
    }

    for (const attr of Array.from(node.attributes)) {
      const name = attr.name;
      const lowerName = name.toLowerCase();
      if (EVENT_ATTR_RE.test(lowerName)) {
        node.removeAttribute(name);
        continue;
      }
      if (URL_ATTRS.has(lowerName) && UNSAFE_URL_RE.test(attr.value)) {
        node.removeAttribute(name);
        continue;
      }
      if (!ALLOWED_SVG_ATTRS.has(lowerName)) {
        node.removeAttribute(name);
      }
    }

    Array.from(node.children).forEach((child) => sanitizeNode(child));
  };

  sanitizeNode(root);
  return root.outerHTML;
}
