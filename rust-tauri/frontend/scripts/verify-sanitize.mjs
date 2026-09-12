import { JSDOM } from "jsdom";
import { marked } from "marked";
import createDOMPurify from "dompurify";

const dom = new JSDOM("");
const DOMPurify = createDOMPurify(dom.window);

const renderer = new marked.Renderer();
const defaultCode = renderer.code.bind(renderer);
renderer.code = function (code, lang, escaped) {
  const language = (lang || "").split(/\s+/)[0];
  if (language === "mermaid") {
    const src = typeof code === "string" ? code : (code && code.text) || "";
    return '<pre class="mermaid">' + escape(src) + "</pre>";
  }
  return defaultCode(code, lang, escaped);
};
marked.use({ renderer });

function escape(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMarkdown(md) {
  const html = marked.parse(md || "");
  return DOMPurify.sanitize(html, {
    ADD_TAGS: ["pre"],
    ADD_ATTR: ["class"],
    FORBID_TAGS: ["style", "iframe", "object", "embed", "form"],
    FORBID_ATTR: ["style"],
  });
}

const hostile = [
  {
    name: "raw script tag",
    md: "hello <script>alert(1)</script>",
    forbidden: ["<script", "alert(1)"],
  },
  {
    name: "img onerror",
    md: 'hello <img src=x onerror="alert(1)">',
    forbidden: ["onerror", "alert(1)"],
  },
  {
    name: "svg onload",
    md: '<svg onload="alert(1)"></svg>',
    forbidden: ["onload", "alert(1)"],
  },
  {
    name: "javascript: href",
    md: '[click](javascript:alert(1))',
    forbidden: ["javascript:"],
  },
  {
    name: "iframe injection",
    md: '<iframe src="https://evil.example"></iframe>',
    forbidden: ["<iframe", "evil.example"],
  },
  {
    name: "form injection",
    md: '<form action=/x><input name=csrf></form>',
    forbidden: ["<form"],
  },
  {
    name: "style attribute",
    md: '<p style="background:url(javascript:alert(1))">x</p>',
    forbidden: ["javascript:", "style="],
  },
];

const benign = [
  {
    name: "heading and paragraph",
    md: "# Title\n\nhello **bold** world",
    required: ["<h1", "Title", "<strong>bold</strong>"],
  },
  {
    name: "code fence stays escaped",
    md: "```\n<script>x</script>\n```",
    required: ["&lt;script&gt;"],
    forbidden: ["<script>"],
  },
  {
    name: "mermaid block preserved",
    md: "```mermaid\ngraph LR; A-->B\n```",
    required: ['<pre class="mermaid">', "A--&gt;B"],
  },
  {
    name: "external link stays",
    md: "[docs](https://example.com/page)",
    required: ["https://example.com/page", "docs"],
  },
];

let failures = 0;

for (const c of hostile) {
  const out = renderMarkdown(c.md);
  const bad = c.forbidden.filter((f) => out.toLowerCase().includes(f.toLowerCase()));
  if (bad.length) {
    console.error(`FAIL hostile[${c.name}]: fragments survived: ${JSON.stringify(bad)}\n  output: ${out}`);
    failures++;
  } else {
    console.log(`ok hostile[${c.name}]`);
  }
}

for (const c of benign) {
  const out = renderMarkdown(c.md);
  const miss = (c.required || []).filter((f) => !out.includes(f));
  if (miss.length) {
    console.error(`FAIL benign[${c.name}]: required fragments missing: ${JSON.stringify(miss)}\n  output: ${out}`);
    failures++;
    continue;
  }
  const stray = (c.forbidden || []).filter((f) => out.includes(f));
  if (stray.length) {
    console.error(`FAIL benign[${c.name}]: forbidden fragments present: ${JSON.stringify(stray)}\n  output: ${out}`);
    failures++;
    continue;
  }
  console.log(`ok benign[${c.name}]`);
}

if (failures) {
  console.error(`${failures} sanitisation cases failed`);
  process.exit(1);
}
console.log("all sanitisation checks passed");
