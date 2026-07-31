import { markdownLineEnding } from 'micromark-util-character';
import type { Code, Effects, Extension, State, TokenizeContext } from 'micromark-util-types';
import type {
  CompileContext,
  Extension as FromMarkdownExtension,
  Token,
} from 'mdast-util-from-markdown';
import type { Data, Literal } from 'mdast';
import type { Processor } from 'unified';

// Character codes used by the tokenizer.
const LEFT_BRACKET = 91; // [
const RIGHT_BRACKET = 93; // ]

/**
 * A `[[wikilink]]` mdast node. It carries the parsed `target`, optional
 * `heading`, and optional display `label`, plus the `data.hName`/`hProperties`
 * that turn it into a `<wiki-link>` hast element with `data-*` attributes.
 */
export interface WikiLink extends Literal {
  type: 'wikiLink';
  target: string;
  heading: string | null;
  label: string | null;
  data?: WikiLinkData;
}

interface WikiLinkData extends Data {
  target?: string;
  heading?: string | null;
  label?: string | null;
}

declare module 'mdast' {
  interface RootContentMap {
    wikiLink: WikiLink;
  }
  interface PhrasingContentMap {
    wikiLink: WikiLink;
  }
}

declare module 'micromark-util-types' {
  interface TokenTypeMap {
    wikiLink: 'wikiLink';
    wikiLinkMarker: 'wikiLinkMarker';
    wikiLinkData: 'wikiLinkData';
  }
}

/**
 * micromark syntax extension: tokenizes `[[ ... ]]` as a single unit *before*
 * the standard link/link-reference machinery can claim the brackets. The inner
 * text (`target#heading|label`) is captured as one token and split later.
 *
 * Malformed openings (unterminated, empty, containing `[`, or crossing a line)
 * cleanly fail (`nok`) so the text falls back to literal rendering — the parser
 * never hangs.
 */
function wikiLinkSyntax(): Extension {
  function tokenize(this: TokenizeContext, effects: Effects, ok: State, nok: State): State {
    let sawContent = false;

    return start;

    function start(code: Code): State | undefined {
      // code === LEFT_BRACKET (guaranteed by the dispatch table).
      effects.enter('wikiLink');
      effects.enter('wikiLinkMarker');
      effects.consume(code);
      return openSecond;
    }

    function openSecond(code: Code): State | undefined {
      if (code !== LEFT_BRACKET) return nok(code);
      effects.consume(code);
      effects.exit('wikiLinkMarker');
      effects.enter('wikiLinkData');
      return inside;
    }

    function inside(code: Code): State | undefined {
      if (code === null || markdownLineEnding(code) || code === LEFT_BRACKET) {
        return nok(code);
      }
      if (code === RIGHT_BRACKET) {
        if (!sawContent) return nok(code);
        effects.exit('wikiLinkData');
        effects.enter('wikiLinkMarker');
        effects.consume(code);
        return closeSecond;
      }
      sawContent = true;
      effects.consume(code);
      return inside;
    }

    function closeSecond(code: Code): State | undefined {
      if (code !== RIGHT_BRACKET) return nok(code);
      effects.consume(code);
      effects.exit('wikiLinkMarker');
      effects.exit('wikiLink');
      return ok;
    }
  }

  return {
    text: {
      [LEFT_BRACKET]: { name: 'wikiLink', tokenize },
    },
  };
}

/** Split `target#heading|label` into its parts. */
function parseWikiTarget(rawInput: string): {
  target: string;
  heading: string | null;
  label: string | null;
} {
  const raw = rawInput.trim();
  let left = raw;
  let label: string | null = null;

  const pipe = raw.indexOf('|');
  if (pipe !== -1) {
    label = raw.slice(pipe + 1).trim() || null;
    left = raw.slice(0, pipe);
  }

  let heading: string | null = null;
  let target = left.trim();
  const hash = left.indexOf('#');
  if (hash !== -1) {
    heading = left.slice(hash + 1).trim() || null;
    target = left.slice(0, hash).trim();
  }

  return { target, heading, label };
}

/** mdast-util-from-markdown extension building {@link WikiLink} nodes. */
function wikiLinkFromMarkdown(): FromMarkdownExtension {
  function enterWikiLink(this: CompileContext, token: Token): void {
    this.enter(
      {
        type: 'wikiLink',
        value: '',
        target: '',
        heading: null,
        label: null,
        data: {},
      } as WikiLink,
      token,
    );
  }

  function exitWikiLinkData(this: CompileContext, token: Token): void {
    const node = this.stack[this.stack.length - 1] as WikiLink | undefined;
    if (node && node.type === 'wikiLink') {
      node.value = this.sliceSerialize(token);
    }
  }

  function exitWikiLink(this: CompileContext, token: Token): void {
    const node = this.stack[this.stack.length - 1] as WikiLink;
    this.exit(token);
    const { target, heading, label } = parseWikiTarget(node.value);
    node.target = target;
    node.heading = heading;
    node.label = label;

    const display = label ?? (heading ? `${target}#${heading}` : target);

    // hast property names are camelCase; `dataTarget` serializes to
    // `data-target` and survives the rehype-raw round-trip as `dataTarget`.
    const hProperties: Record<string, string | string[]> = {
      className: ['wikilink'],
      dataWikilink: 'true',
      dataTarget: target,
    };
    if (heading) hProperties.dataHeading = heading;
    if (label) hProperties.dataLabel = label;

    node.data = {
      target,
      heading,
      label,
      hName: 'wiki-link',
      hProperties,
      hChildren: [{ type: 'text', value: display }],
    };
  }

  return {
    enter: { wikiLink: enterWikiLink },
    exit: { wikiLinkData: exitWikiLinkData, wikiLink: exitWikiLink },
  };
}

/**
 * remark plugin registering the wikilink micromark + fromMarkdown extensions.
 */
export function remarkWikiLink(this: Processor): undefined {
  const data = this.data();

  const micromarkExtensions = (data.micromarkExtensions ??= []);
  const fromMarkdownExtensions = (data.fromMarkdownExtensions ??= []);

  micromarkExtensions.push(wikiLinkSyntax());
  fromMarkdownExtensions.push(wikiLinkFromMarkdown());
}
