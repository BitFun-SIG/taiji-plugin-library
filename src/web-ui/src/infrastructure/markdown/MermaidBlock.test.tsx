/**
 * @vitest-environment jsdom
 */

import { describe, expect, it } from 'vitest';

import { sanitizeMermaidSvg } from './sanitizeMermaidSvg';

describe('sanitizeMermaidSvg', () => {
  describe('XSS vectors are removed', () => {
    it('strips <script> elements entirely', () => {
      const raw = '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><circle cx="1" cy="1" r="1"/></svg>';
      const clean = sanitizeMermaidSvg(raw);
      expect(clean).not.toContain('<script');
      expect(clean).not.toContain('alert(1)');
      expect(clean.toLowerCase()).toContain('<svg');
      expect(clean.toLowerCase()).toContain('<circle');
    });

    it('strips on* event handlers from elements', () => {
      const raw = '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><g onmouseover="alert(2)"><circle cx="1" cy="1" r="1"/></g></svg>';
      const clean = sanitizeMermaidSvg(raw);
      expect(clean).not.toContain('onload');
      expect(clean).not.toContain('onmouseover');
      expect(clean).not.toContain('alert(2)');
      expect(clean.toLowerCase()).toContain('<svg');
    });

    it('strips javascript: URLs from href/src/xlink:href', () => {
      const raw = '<svg xmlns="http://www.w3.org/2000/svg"><a href="javascript:alert(1)">x</a><image xlink:href="javascript:alert(2)"/></svg>';
      const clean = sanitizeMermaidSvg(raw);
      expect(clean).not.toContain('javascript:');
      expect(clean.toLowerCase()).toContain('<svg');
    });

    it('removes injected HTML/script inside foreignObject labels', () => {
      const raw =
        '<svg xmlns="http://www.w3.org/2000/svg"><foreignObject width="10" height="10">' +
        '<div xmlns="http://www.w3.org/1999/xhtml"><img src="x" onerror="alert(1)">' +
        '<script>alert(2)</script><p onclick="alert(3)">label</p></div></foreignObject></svg>';
      const clean = sanitizeMermaidSvg(raw);
      expect(clean).not.toContain('onerror');
      expect(clean).not.toContain('onclick');
      expect(clean).not.toContain('<script');
      expect(clean).not.toContain('alert(1)');
      expect(clean).not.toContain('<img');
      expect(clean.toLowerCase()).toContain('label');
    });

    it('returns empty string for malformed / non-svg input', () => {
      expect(sanitizeMermaidSvg('')).toBe('');
      expect(sanitizeMermaidSvg('<div>not an svg</div>')).toBe('');
    });
  });

  describe('normal mermaid output is preserved', () => {
    it('keeps structural svg elements and drops nothing that a rendered diagram needs', () => {
      const raw = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="500" height="300" viewBox="0 0 500 300" role="img" aria-roledescription="diagram">
  <style>#mermaid-1 .node rect { fill: #fff; }</style>
  <defs>
    <marker id="mermaid-1_arrow" markerWidth="10" markerHeight="10" orient="auto" refX="9" refY="3">
      <path d="M0,0 L0,6 L9,3 z" fill="black"/>
    </marker>
    <linearGradient id="mermaid-1_g" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="#ffffff"/>
      <stop offset="100%" stop-color="#eeeeee"/>
    </linearGradient>
  </defs>
  <g id="mermaid-1" class="node" transform="translate(100,50)">
    <rect width="120" height="40" rx="5" ry="5" fill="url(#mermaid-1_g)" stroke="#333" stroke-width="1.5"/>
    <text x="60" y="25" text-anchor="middle" dominant-baseline="central" font-size="14" font-family="sans-serif">Process</text>
  </g>
  <g id="mermaid-1_label" class="edgeLabel">
    <foreignObject width="100" height="30" x="0" y="0">
      <div xmlns="http://www.w3.org/1999/xhtml" class="nodeLabel" style="text-align:center;"><p>Hello</p></div>
    </foreignObject>
  </g>
</svg>`;
      const clean = sanitizeMermaidSvg(raw);
      const lower = clean.toLowerCase();
      expect(lower).toContain('<svg');
      expect(lower).toContain('<path');
      expect(lower).toContain('<rect');
      expect(lower).toContain('<text');
      expect(lower).toContain('<foreignobject');
      expect(lower).toContain('<div');
      expect(lower).toContain('<style');
      expect(lower).toContain('class="nodelabel"');
      expect(lower).toContain('viewbox');
      // safety invariants on a normal diagram
      expect(clean).not.toContain('<script');
      expect(clean).not.toMatch(/on\w+=/i);
    });
  });
});
