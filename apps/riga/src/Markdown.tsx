import { memo, useCallback, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Fenced blocks carry `language-x`; inline code carries no class. */
function languageOf(className: unknown): string | null {
  if (typeof className !== "string") return null;
  const match = /language-([\w+-]+)/.exec(className);
  return match ? match[1] : null;
}

/**
 * Give every ``` run its own line.
 *
 * CommonMark only opens a code fence at the start of a line, so a model that
 * writes "…set up the server: ```javascript" gets the whole block back as
 * literal text: the backticks, the language tag, and the code, all unrendered in
 * one run-on paragraph. That is exactly what models do when a transport has
 * flattened the newlines out of a reply, so the fence is repaired here instead.
 *
 * Two rules keep it safe. A newline is only ever *inserted* where the source did
 * not already have one, so input that is already valid is returned untouched.
 * And an opening fence keeps whatever follows it on the same line, because that
 * is where the info string (`javascript`) belongs; only a closing fence is
 * forced to end its line.
 */
export function normalizeFences(text: string): string {
  let out = "";
  let index = 0;
  let insideFence = false;
  for (;;) {
    const start = text.indexOf("```", index);
    if (start < 0) break;
    let end = start;
    while (end < text.length && text[end] === "`") end += 1;
    out += text.slice(index, start);
    // A fence must own its line, so break away from any trailing prose.
    if (out.length > 0 && !out.endsWith("\n")) out += "\n";
    out += text.slice(start, end);
    index = end;
    if (insideFence) {
      insideFence = false;
      // Anything after a closing fence starts new prose and must be on a new line.
      if (index < text.length && text[index] !== "\n") out += "\n";
    } else {
      insideFence = true;
    }
  }
  return out + text.slice(index);
}

/** Copy button for a fenced block. Uses the clipboard API, so it is browser-only. */
function CodeBlock({ children, className }: { children?: ReactNode; className?: string }) {
  const [copied, setCopied] = useState(false);
  const language = languageOf(className);
  // react-markdown hands the already-stringified code as a single child here.
  const text = typeof children === "string" ? children : "";
  const copy = useCallback(() => {
    void navigator.clipboard
      .writeText(text.replace(/\n$/, ""))
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1400);
      })
      // A denied clipboard permission should not surface as an unhandled
      // rejection; the button simply does not confirm.
      .catch(() => undefined);
  }, [text]);
  return (
    <div className="code-block">
      <div className="code-block-bar">
        <span>{language ?? "text"}</span>
        {text ? (
          <button type="button" onClick={copy}>
            {copied ? "Copied" : "Copy"}
          </button>
        ) : null}
      </div>
      <pre>
        <code className={className}>{children}</code>
      </pre>
    </div>
  );
}

/**
 * External links get `target`/`rel`; without `rel="noreferrer"` a model-supplied
 * link leaks the opener to the destination. `noopener` covers older browsers.
 */
function linkTarget(href: string | undefined): { target?: string; rel?: string } {
  if (!href || !/^https?:/i.test(href)) return {};
  return { target: "_blank", rel: "noopener noreferrer" };
}

const components = {
  pre({ children }: { children?: ReactNode }) {
    // Unwrap the <pre> react-markdown adds so CodeBlock owns the frame; keeping
    // both would nest a bordered box inside another one.
    const child = Array.isArray(children) ? children[0] : children;
    if (child && typeof child === "object" && "props" in child) {
      const props = (child as { props: { className?: string; children?: ReactNode } }).props;
      return <CodeBlock className={props.className}>{props.children}</CodeBlock>;
    }
    return <pre>{children}</pre>;
  },
  a({ href, children }: { href?: string; children?: ReactNode }) {
    return <a href={href} {...linkTarget(href)}>{children}</a>;
  },
};

/**
 * Renders model prose as markdown.
 *
 * `react-markdown` builds React elements from the parsed AST rather than
 * injecting HTML, so a reply containing `<script>` or a `javascript:` link is
 * rendered as inert text. `rehype-raw` is deliberately not installed: without
 * it, raw HTML in a reply is escaped instead of parsed, which is what we want
 * for content that originates from a tool-using model.
 *
 * A model also emits unterminated fences mid-stream, so the parser has to
 * survive partial input rather than dropping the message; `normalizeFences`
 * covers the related case of a fence that lost its line breaks entirely.
 *
 * Memoized because a streaming reply re-renders on every token: without this,
 * the whole transcript re-parses whenever any sibling state changes.
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {normalizeFences(text)}
      </ReactMarkdown>
    </div>
  );
});
