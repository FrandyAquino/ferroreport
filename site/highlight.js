/* Syntax colors for the site's code blocks. A block says its language: <pre data-lang="shell">.
 * Several languages, separated by blank lines, are given in order ("json shell": the first part is
 * JSON, the rest shell). Only the text is read; the colors are <span class="tk-..."> wrapped around
 * the text nodes already there, so links inside a block keep working, entities stay as they are,
 * and copying gives the plain text. Run again on a block (ferroHighlight(pre)) after changing it.
 * The playground's editors (pre.hl) have their own colors and no data-lang, so they are left alone. */
(() => {
  "use strict";
  // Rules are tried in order at each position; `when` checks what comes before the match.
  const W = /[A-Za-z0-9_$]/;
  const blankBefore = (text, i) => i === 0 || /\s/.test(text[i - 1]);

  const COMMON = {
    dq: { re: /"(?:[^"\\\n]|\\.)*"/y, cls: "s" },
    sq: { re: /'(?:[^'\\\n]|\\.)*'/y, cls: "s" },
    num: { re: /\b(?:0x[0-9a-fA-F]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?[LlFfDdMm]?)\b/y, cls: "n" },
    lineComment: { re: /\/\/[^\n]*/y, cls: "c" },
    blockComment: { re: /\/\*[\s\S]*?(?:\*\/|$)/y, cls: "c" },
    hashComment: { re: /#[^\n]*/y, cls: "c", when: blankBefore },
    call: { re: /[A-Za-z_][\w]*(?=\s*\()/y, cls: "f" },
    type: { re: /\b[A-Z][a-z0-9]\w*\b/y, cls: "t" },
    jasper: { re: /\$[FPVRX]!?\{[^}\n]*\}/y, cls: "v" },
  };
  // Keywords, but not as a property (Buffer.from).
  const words = (list, cls) => ({ re: new RegExp(`\\b(?:${list.split(" ").join("|")})\\b`, "y"), cls, when: (t, i) => t[i - 1] !== "." });

  const JAVA_KW = "abstract assert break case catch class continue default do else enum extends final finally for if implements import instanceof interface new package private protected public return static super switch this throw throws try var void while byte short int long float double boolean char";
  const LANGS = {
    js: [
      COMMON.lineComment, COMMON.blockComment, COMMON.dq, COMMON.sq,
      { re: /`(?:[^`\\]|\\.)*`/y, cls: "s" },
      words("import from export default const let var function return await async new if else for of in while throw try catch class extends typeof instanceof this", "k"),
      words("true false null undefined", "n"),
      COMMON.num, COMMON.call, COMMON.type,
    ],
    python: [
      COMMON.hashComment,
      { re: /[rbfu]{0,2}("""|''')[\s\S]*?(?:\1|$)/y, cls: "s" },
      { re: /[rbfu]{0,2}(?:"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*')/y, cls: "s" },
      { re: /@[A-Za-z_][\w.]*/y, cls: "k", when: (t, i) => i === 0 || /\n\s*$/.test(t.slice(Math.max(0, i - 80), i)) },
      words("import from as def return for in if elif else while with class lambda try except finally raise yield and or not is pass", "k"),
      words("True False None", "n"),
      COMMON.num, COMMON.call, COMMON.type,
    ],
    java: [
      COMMON.lineComment, COMMON.blockComment, COMMON.dq,
      { re: /'(?:[^'\\\n]|\\.)'/y, cls: "s" },
      COMMON.jasper,
      { re: /@[A-Za-z_]\w*/y, cls: "k" },
      words(JAVA_KW, "k"),
      words("true false null", "n"),
      COMMON.num, COMMON.call, COMMON.type,
    ],
    groovy: [
      COMMON.lineComment, COMMON.blockComment, COMMON.dq, COMMON.sq, COMMON.jasper,
      words("def in as new return if else it", "k"),
      words("true false null", "n"),
      COMMON.num, COMMON.call, COMMON.type,
    ],
    csharp: [
      COMMON.lineComment, COMMON.blockComment,
      { re: /[@$]?"(?:[^"\\\n]|\\.)*"/y, cls: "s" },
      words("using var new await async return if else class public private static void byte int string object", "k"),
      words("true false null", "n"),
      COMMON.num, COMMON.call, COMMON.type,
    ],
    php: [
      COMMON.lineComment, COMMON.blockComment, COMMON.hashComment, COMMON.dq, COMMON.sq,
      { re: /\$[A-Za-z_]\w*/y, cls: "v" },
      { re: /\b[A-Z][A-Z0-9_]+\b/y, cls: "t" },
      words("new return if else function echo array", "k"),
      words("true false null", "n"),
      COMMON.num, COMMON.call,
    ],
    rust: [
      COMMON.lineComment, COMMON.blockComment, COMMON.dq,
      { re: /b?'(?:[^'\\\n]|\\.)'/y, cls: "s" },
      words("use let mut fn pub struct enum impl trait for in if else match return where as move ref self Self crate super mod const static loop while async await dyn", "k"),
      words("true false None Some Ok Err", "n"),
      { re: /[A-Za-z_]\w*!(?=[(\[{])/y, cls: "f" },
      COMMON.num, COMMON.call, COMMON.type,
    ],
    json: [
      COMMON.hashComment, COMMON.lineComment,
      { re: /"(?:[^"\\\n]|\\.)*"(?=\s*:)/y, cls: "a" },
      COMMON.dq,
      words("true false null", "n"),
      { re: /-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b/y, cls: "n" },
      { re: /[{}[\],:]/y, cls: "p" },
    ],
  };
  LANGS.ts = LANGS.javascript = LANGS.js;
  LANGS.py = LANGS.python;
  LANGS.cs = LANGS.csharp;

  // Runs the rules of a language over text[from, to) and returns [start, end, class] tokens.
  function scan(rules, text, from, to, out) {
    let i = from;
    outer: while (i < to) {
      for (const r of rules) {
        r.re.lastIndex = i;
        const m = r.re.exec(text);
        if (m && m[0].length && m.index === i && (!r.when || r.when(text, i))) {
          const end = Math.min(i + m[0].length, to);
          if (r.cls) out.push([i, end, r.cls]);
          i = end;
          continue outer;
        }
      }
      // Nothing matched: skip a whole word, so that keywords never match inside one.
      if (W.test(text[i])) { while (i < to && W.test(text[i])) i++; } else i++;
    }
    return out;
  }

  // ---------- shell ----------
  // Is position i where a command name goes (start of a line, or after |, ;, &&, {, or VAR=value)?
  function commandPosition(text, i) {
    const lineStart = text.lastIndexOf("\n", i - 1) + 1;
    // Quoted text cannot hold a separator; a "$ " prompt is not part of the command.
    let before = text.slice(lineStart, i).replace(/'[^']*'|"(?:[^"\\]|\\.)*"/g, '""').replace(/^\s*\$ /, "");
    const sep = Math.max(before.lastIndexOf("|"), before.lastIndexOf(";"), before.lastIndexOf("&"), before.lastIndexOf("{"), before.lastIndexOf("("));
    if (sep >= 0) before = before.slice(sep + 1);
    else if (lineStart > 0) {
      // A line continued with a backslash goes on with the command, unless it ended with a pipe.
      const prev = text.slice(text.lastIndexOf("\n", lineStart - 2) + 1, lineStart - 1).replace(/\s+$/, "");
      if (prev.endsWith("\\") && !/[|&;{]\s*\\$/.test(prev)) return false;
    }
    return /^\s*(?:[A-Za-z_]\w*=\S*\s+)*$/.test(before);
  }
  LANGS.shell = [
    COMMON.hashComment,
    { re: /'[^']*'/y, cls: "s" },
    { re: /"(?:[^"\\]|\\.)*"/y, cls: "s" },
    { re: /\$\{[^}\n]*\}|\$[A-Za-z_]\w*/y, cls: "v" },
    { re: /[A-Za-z_]\w*=/y, cls: "v", when: commandPosition },
    { re: /[A-Za-z_.][\w.-]*/y, cls: "f", when: commandPosition },
    { re: /--?[A-Za-z][\w-]*/y, cls: "a", when: blankBefore },
    { re: /[|&;<>]+|\\(?=\n)/y, cls: "p" },
  ];
  LANGS.bash = LANGS.sh = LANGS.shell;

  // A terminal session: "$ command" lines in shell colors, the rest is what the program printed.
  function console_(text, from, to, out) {
    const re = /^[ \t]*\$ [^\n]*/gm;
    re.lastIndex = from;
    let m;
    while ((m = re.exec(text)) && m.index < to) {
      const d = m.index + m[0].indexOf("$");
      out.push([d, d + 1, "p"]);
      scan(LANGS.shell, text, d + 2, Math.min(m.index + m[0].length, to), out);
    }
    return out;
  }

  // A command's synopsis: name, subcommand, <placeholders>, [optional parts], --options.
  LANGS.usage = [
    { re: /^[\w.-]+/my, cls: "f" },
    { re: /<[^>\n]+>/y, cls: "v" },
    { re: /--?[A-Za-z][\w-]*/y, cls: "a", when: (t, i) => i === 0 || /[\s[]/.test(t[i - 1]) },
    { re: /[[\]|.]+/y, cls: "p" },
  ];

  // ---------- XML / JRXML ----------
  function xml(text, from, to, out) {
    const re = /<!--[\s\S]*?(?:-->|$)|<!\[CDATA\[[\s\S]*?(?:\]\]>|$)|<\?[\s\S]*?\?>|<\/?[\w:.-]+|#[^\n]*/g;
    const tagEnd = /(?:\s+[\w:.-]+(?:\s*=\s*(?:"[^"]*"|'[^']*'))?)*\s*(\/?>)/y;
    re.lastIndex = from;
    let m;
    while ((m = re.exec(text)) && m.index < to) {
      const s = m.index, e = Math.min(s + m[0].length, to), tok = m[0];
      if (tok.startsWith("<!--") || tok.startsWith("<?")) out.push([s, e, "c"]);
      else if (tok.startsWith("<![CDATA[")) {
        // The markers are punctuation; what is inside is a Java expression.
        const closed = tok.endsWith("]]>") && e === s + tok.length;
        out.push([s, Math.min(s + 9, e), "p"]);
        scan(LANGS.java, text, s + 9, closed ? e - 3 : e, out);
        if (closed) out.push([e - 3, e, "p"]);
      } else if (tok[0] === "<") {
        const lt = tok[1] === "/" ? 2 : 1;
        out.push([s, s + lt, "p"], [s + lt, e, "t"]);
        tagEnd.lastIndex = e;
        const t = tagEnd.exec(text);
        if (t && t.index === e) {
          const close = e + t[0].length - t[1].length;
          attrs(text, e, Math.min(close, to), out);
          if (close < to) out.push([close, Math.min(close + t[1].length, to), "p"]);
          re.lastIndex = e + t[0].length;
        }
      } else if (blankBefore(text, s)) {
        // A note written after a line of markup in these pages ("... # read from lines.jrxml").
        out.push([s, e, "c"]);
      } else re.lastIndex = s + 1;
    }
    return out;
  }
  function attrs(text, from, to, out) {
    const re = /([\w:.-]+)(\s*=\s*)("[^"]*"|'[^']*')?/g;
    re.lastIndex = from;
    let m;
    while ((m = re.exec(text)) && m.index < to) {
      const s = m.index;
      out.push([s, s + m[1].length, "a"]);
      if (m[3]) {
        const v = s + m[1].length + m[2].length;
        out.push([v, Math.min(v + m[3].length, to), "s"]);
      }
    }
  }

  // ---------- .ferro ----------
  // An indented outline: the first word of a line says what it is, then positions, sizes and
  // name=value attributes, then " : " and its text or expression. "//" lines are comments.
  const HOLES = /\\[{}$=\\]|\{\{\w+\}\}|\$[FPVR]!?\{[^}\n]*\}|\{[A-Za-z_][\w.]*\}/y;
  const NAMED = new Set(["field", "parameter", "variable", "style", "group", "def", "use", "property", "import"]);
  const TEXTS = new Set(["text", "staticText", "h1", "h2", "h3", "h4", "h5", "h6"]);
  const EXPR = [
    COMMON.dq, { re: HOLES, cls: "v" }, words("true false null", "n"), COMMON.num, COMMON.call,
  ];
  function textPart(text, s, e, out) {
    for (let i = s; i < e;) {
      HOLES.lastIndex = i;
      const m = HOLES.exec(text);
      if (m && m.index === i) {
        out.push([i, i + m[0].length, m[0][0] === "\\" ? "p" : "v"]);
        i += m[0].length;
      } else i++;
    }
  }
  function ferro(text, from, to, out) {
    let blockIndent = -1, blockText = true;   // the lines under a line that ends in " :"
    for (let ls = from; ls < to;) {
      let le = text.indexOf("\n", ls);
      if (le < 0 || le > to) le = to;
      const line = text.slice(ls, le), indent = line.match(/^[ \t]*/)[0].length, body = line.slice(indent);
      if (blockIndent >= 0 && body && indent > blockIndent) {
        if (blockText) textPart(text, ls + indent, le, out); else scan(EXPR, text, ls + indent, le, out);
        ls = le + 1; continue;
      }
      blockIndent = -1;
      if (body.startsWith("//")) { out.push([ls + indent, le, "c"]); ls = le + 1; continue; }
      if (!body) { ls = le + 1; continue; }
      const kind = body.match(/^[^\s:]+/);
      const ks = ls + indent, ke = ks + (kind ? kind[0].length : 0);
      if (kind) out.push([ks, ke, "t"]);
      // Where the text starts: " : ", " := ", or a line that ends in " :".
      const colon = /(^|\s):=?(?=\s|$)/g;
      colon.lastIndex = ke - ls;
      const cm = colon.exec(line);
      const headEnd = cm ? ls + cm.index + cm[1].length : le;
      head(text, ke, headEnd, kind ? kind[0] : "", out);
      if (cm) {
        const cs = headEnd, ce = cs + cm[0].length - cm[1].length;
        out.push([cs, ce, "p"]);
        const ts = ce, isText = kind && TEXTS.has(kind[0]) && text[ce - 1] !== "=";
        const rest = text.slice(ts, le);
        if (!rest.trim()) { blockIndent = indent; blockText = isText; }
        else if (isText && !/^\s*=/.test(rest)) textPart(text, ts, le, out);
        else scan(EXPR, text, ts, le, out);
      }
      ls = le + 1;
    }
    return out;
  }
  function head(text, s, e, kind, out) {
    const re = /"(?:[^"\\\n]|\\.)*"|([\w.:-]+)(=)("(?:[^"\\\n]|\\.)*"|\S*)|\S+/g;
    re.lastIndex = s;
    let m, n = 0;
    while ((m = re.exec(text)) && m.index < e) {
      const p = m.index, tok = m[0];
      n++;
      if (tok[0] === '"') out.push([p, p + tok.length, "s"]);
      else if (m[2]) {
        out.push([p, p + m[1].length, "a"]);
        const v = p + m[1].length + 1;
        if (m[3]) out.push([v, v + m[3].length, /^[{$]/.test(m[3]) ? "v" : /^[\d#.,%x-]+$/.test(m[3]) ? "n" : "s"]);
      } else if (/^\d[\d.,x%]*(?:fr)?$/.test(tok)) out.push([p, p + tok.length, "n"]);
      else if (NAMED.has(kind) && n === 1) out.push([p, p + tok.length, "v"]);
      else if (/^[A-Z]/.test(tok) && (kind === "field" || kind === "parameter" || kind === "variable")) out.push([p, p + tok.length, "k"]);
      else if (!NAMED.has(kind)) out.push([p, p + tok.length, "a"]);
    }
  }

  const FUNCS = {
    xml: xml, jrxml: xml, html: xml, ferro: ferro, console: console_,
  };

  function tokenize(text, langs) {
    const out = [];
    // Several languages: one per part, parts separated by blank lines; the last one goes on.
    const parts = [];
    if (langs.length > 1) {
      const re = /\n[ \t]*\n/g;
      let m, start = 0;
      while ((m = re.exec(text))) { parts.push([start, m.index + m[0].length]); start = m.index + m[0].length; }
      parts.push([start, text.length]);
    } else parts.push([0, text.length]);
    parts.forEach(([s, e], k) => {
      const lang = langs[Math.min(k, langs.length - 1)];
      if (FUNCS[lang]) FUNCS[lang](text, s, e, out);
      else if (LANGS[lang]) scan(LANGS[lang], text, s, e, out);
    });
    return out.sort((a, b) => a[0] - b[0]);
  }

  // Wraps the tokens' ranges of the block's text nodes in spans, leaving every other node as it is.
  function highlight(pre) {
    if (!pre || !pre.dataset || !pre.dataset.lang) return;
    pre.querySelectorAll("span[data-tk]").forEach((s) => s.replaceWith(...s.childNodes));
    pre.normalize();
    const text = pre.textContent;
    const toks = tokenize(text, pre.dataset.lang.trim().split(/\s+/));
    if (!toks.length) return;
    const walker = document.createTreeWalker(pre, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    let pos = 0, t = 0;
    for (const node of nodes) {
      const s = pos, e = pos + node.data.length;
      pos = e;
      while (t < toks.length && toks[t][1] <= s) t++;
      if (t >= toks.length || toks[t][0] >= e) continue;
      const frag = document.createDocumentFragment();
      let at = s;
      for (let k = t; k < toks.length && toks[k][0] < e; k++) {
        const [ts, te, cls] = toks[k];
        const a = Math.max(ts, at), b = Math.min(te, e);
        if (b <= a) continue;
        if (a > at) frag.append(node.data.slice(at - s, a - s));
        const span = document.createElement("span");
        span.className = "tk-" + cls;
        span.dataset.tk = "";
        span.textContent = node.data.slice(a - s, b - s);
        frag.append(span);
        at = b;
      }
      if (at < e) frag.append(node.data.slice(at - s));
      node.replaceWith(frag);
    }
  }

  window.ferroHighlight = highlight;
  const all = () => document.querySelectorAll("pre[data-lang]").forEach(highlight);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", all);
  else all();
})();
