import { EditorView } from '@codemirror/view';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import type { Extension } from '@codemirror/state';
import type { Theme } from '../../generated/protocol';
import { readableColor } from '../astryx-theme';

export interface EditorAppearance {
  theme: Theme;
  fontFamily: string;
  fontSize: number;
  fontWeight: number;
  /** Let wallpapers show through, as terminals do. */
  transparent: boolean;
  dark: boolean;
}

/**
 * Token colors in the base16 roles `buildSyntaxStyle` (lib/syntax.ts) uses for
 * file previews: red=base08, brightRed=base09, yellow=base0A, green=base0B,
 * cyan=base0C, blue=base0D, magenta=base0E, brightMagenta=base0F.
 */
export function highlightSpecs(theme: Theme) {
  return [
    { tag: [t.comment, t.quote], color: theme.brightBlack, fontStyle: 'italic' },
    { tag: [t.variableName, t.tagName, t.deleted, t.self, t.labelName], color: theme.red },
    { tag: [t.number, t.bool, t.null, t.atom, t.url, t.constant(t.variableName)], color: theme.brightRed },
    { tag: [t.standard(t.variableName), t.special(t.variableName)], color: theme.brightRed },
    { tag: [t.typeName, t.className, t.namespace, t.attributeName, t.annotation], color: theme.yellow },
    { tag: [t.string, t.special(t.string), t.inserted, t.character], color: theme.green },
    { tag: [t.regexp, t.escape, t.operator, t.special(t.brace)], color: theme.cyan },
    {
      tag: [t.function(t.variableName), t.function(t.propertyName), t.heading, t.macroName],
      color: theme.blue,
    },
    { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.definitionKeyword, t.modifier], color: theme.magenta },
    { tag: [t.meta, t.processingInstruction, t.documentMeta], color: theme.brightMagenta },
    { tag: [t.propertyName, t.definition(t.propertyName)], color: theme.foreground },
    { tag: t.heading, fontWeight: 'bold' },
    { tag: t.emphasis, fontStyle: 'italic' },
    { tag: t.strong, fontWeight: 'bold' },
    { tag: t.strikethrough, textDecoration: 'line-through' },
    { tag: t.link, textDecoration: 'underline' },
    { tag: t.invalid, color: theme.red, textDecoration: 'underline wavy' },
  ];
}

type IconRole = 'blue' | 'cyan' | 'green' | 'brightRed' | 'magenta' | 'red' | 'yellow' | 'foreground';

/**
 * Completion kind icons: mini.icons' LSP codicons, as the Neovim surface shows
 * them, in base16 roles for its highlight groups (Azure/Blue=blue, Orange=brightRed,
 * Purple=magenta, Grey=foreground). Types are completion types (`KIND_TYPES` in
 * completion.ts, plus CodeMirror's own `namespace` and `type`).
 */
const KIND_ICONS: [type: string, glyph: string, role: IconRole][] = [
  ['text', '\uea93', 'green'],
  ['method', '\uea8c', 'blue'],
  ['function', '\ueabe', 'blue'],
  ['constructor', '\ueaf8', 'blue'],
  ['field', '\ueb5f', 'yellow'],
  ['variable', '\uea88', 'cyan'],
  ['class', '\ueb5b', 'magenta'],
  ['interface', '\ueb61', 'magenta'],
  ['module', '\uea8b', 'magenta'],
  ['namespace', '\uea8b', 'red'],
  ['property', '\ueb65', 'yellow'],
  ['unit', '\uea96', 'cyan'],
  ['value', '\ueaac', 'blue'],
  ['enum', '\uea95', 'magenta'],
  ['keyword', '\ueb62', 'cyan'],
  ['snippet', '\ueb66', 'green'],
  ['color', '\ueb5c', 'red'],
  ['file', '\ueb60', 'blue'],
  ['reference', '\ueb36', 'cyan'],
  ['folder', '\uea83', 'blue'],
  ['enumMember', '\ueb5e', 'yellow'],
  ['constant', '\ueb5d', 'brightRed'],
  ['struct', '\uea91', 'magenta'],
  ['event', '\uea86', 'red'],
  ['operator', '\ueb64', 'cyan'],
  ['typeParameter', '\uea92', 'cyan'],
  ['type', '\uea92', 'cyan'],
];

function completionIcons(theme: Theme) {
  const rules: Record<string, Record<string, string>> = {
    // CodeMirror's list is plain `monospace`; the editor font falls back to the bundled Nerd Font.
    '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontFamily: 'inherit' },
    '.cm-completionIcon': { width: '1em', opacity: '1' },
  };
  for (const [type, glyph, role] of KIND_ICONS) {
    // CodeMirror's `abc` for text is half size and centered.
    rules[`.cm-completionIcon-${type}:after`] = {
      content: `'${glyph}'`,
      color: theme[role],
      fontSize: '100%',
      verticalAlign: 'baseline',
    };
    // The glyph is the row's only kind cue: keep it legible on the selection.
    rules[`.cm-tooltip-autocomplete > ul > li[aria-selected] .cm-completionIcon-${type}:after`] = {
      color: readableColor(theme[role], theme.selectionBackground, 3),
    };
  }
  return rules;
}

/** The native editor's popups: half translucent, blurred by `backdrop-blur`. */
export const POPUP_GLASS = {
  backgroundColor: 'color-mix(in srgb, var(--color-background-popover) 50%, transparent)',
  backdropFilter: 'blur(var(--btm-backdrop-blur, 2px))',
} as const;

/** The editor's look, from the terminal palette and font. */
export function editorTheme(a: EditorAppearance): Extension {
  const { theme } = a;
  const background = a.transparent ? 'transparent' : theme.background;
  const font = { fontFamily: a.fontFamily, fontSize: `${a.fontSize}px`, fontWeight: String(a.fontWeight) };
  return [
    EditorView.theme(
      {
        '&': { color: theme.foreground, backgroundColor: background, height: '100%', ...font },
        '&.cm-focused': { outline: 'none' },
        '.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.45' },
        '.cm-content': { caretColor: theme.cursor },
        '.cm-cursor, .cm-dropCursor': { borderLeftColor: theme.cursor, borderLeftWidth: '2px' },
        // Vim's block cursor; partial ones (pending operator, replace) have no letter.
        '& .cm-vimCursorLayer .cm-fat-cursor': { background: theme.cursor },
        '& .cm-vimCursorLayer .cm-fat-cursor:not([style*="transparent"])': {
          color: `${theme.cursorAccent} !important`,
        },
        '&:not(.cm-focused) .cm-vimCursorLayer .cm-fat-cursor': {
          background: 'none',
          outline: `1px solid ${theme.cursor}`,
          color: 'transparent !important',
        },
        // A `:` or `/` prompt leaves vim's status pending, which halves the
        // cursor (the bottom half of the line); keep the whole cell outlined.
        '&:has(.cm-vim-panel):not(.cm-focused) .cm-vimCursorLayer .cm-fat-cursor[style*="transparent"]': {
          outline: 'none',
        },
        '&:has(.cm-vim-panel):not(.cm-focused) .cm-vimCursorLayer .cm-fat-cursor[style*="transparent"]::before': {
          content: '""',
          position: 'absolute',
          inset: '-100% 0 0 0',
          outline: `1px solid ${theme.cursor}`,
        },
        '.cm-vim-panel': { fontFamily: 'inherit', padding: '2px 8px' },
        '.cm-vim-panel input': { font: 'inherit', color: 'inherit', backgroundColor: 'transparent' },
        '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection':
          { backgroundColor: theme.selectionBackground },
        '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--btm-editor-fg) 5%, transparent)' },
        '.cm-selectionMatch': { backgroundColor: 'color-mix(in srgb, var(--btm-editor-blue) 25%, transparent)' },
        '.cm-searchMatch': {
          backgroundColor: 'color-mix(in srgb, var(--btm-editor-yellow) 30%, transparent)',
          outline: `1px solid color-mix(in srgb, ${theme.yellow} 60%, transparent)`,
        },
        '.cm-searchMatch.cm-searchMatch-selected': {
          backgroundColor: 'color-mix(in srgb, var(--btm-editor-yellow) 55%, transparent)',
        },
        '&.cm-focused .cm-matchingBracket': {
          backgroundColor: 'color-mix(in srgb, var(--btm-editor-cyan) 30%, transparent)',
        },
        '&.cm-focused .cm-nonmatchingBracket': { color: theme.red },
        '.cm-gutters': { backgroundColor: background, color: theme.brightBlack, border: 'none' },
        '.cm-activeLineGutter': { backgroundColor: 'transparent', color: theme.foreground },
        '.cm-foldPlaceholder': {
          backgroundColor: 'transparent',
          border: `1px solid ${theme.brightBlack}`,
          color: theme.brightBlack,
        },
        // Popups are translucent, blurring what's behind them like Neovim's
        // floats (`POPUP_GLASS`).
        '.cm-tooltip, .cm-panels': {
          ...POPUP_GLASS,
          color: theme.foreground,
          border: '1px solid var(--color-border)',
          borderRadius: '6px',
        },
        // Panels (quick open, pickers, `:` and `/`) float over the text, so
        // it shows through; `panelSpace` keeps the cursor clear of them.
        '.cm-panels': { position: 'absolute' },
        // Close buttons inherit the background, which would stack.
        '.cm-panels [name=close]': { backgroundColor: 'transparent' },
        // CodeMirror shrinks a tooltip that doesn't fit to the space left;
        // its documentation scrolls there rather than spilling out.
        '.cm-tooltip.cm-tooltip-hover, .cm-tooltip.cm-lsp-hover-tooltip': {
          maxWidth: 'min(90ch, 90vw)',
          maxHeight: '30em',
          overflow: 'auto',
        },
        // CodeMirror caps completion docs' width only.
        '.cm-tooltip.cm-completionInfo': { maxHeight: 'min(30em, 70vh)', overflow: 'auto' },
        // Its `pre-line` is for plain text; in rendered markdown it would
        // turn the newlines between blocks into blank lines.
        '.cm-completionInfo .cm-lsp-documentation:has(> *)': { whiteSpace: 'normal' },
        '.cm-panels.cm-panels-bottom': { borderRadius: 0, borderWidth: '1px 0 0 0' },
        '.cm-panels input, .cm-panels button': { font: 'inherit' },
        '.cm-tooltip-autocomplete > ul > li[aria-selected]': {
          backgroundColor: theme.selectionBackground,
          color: theme.foreground,
        },
        // Highlighted as code (`codeDetail`), not set apart by italics.
        '.cm-completionDetail.cm-btmux-completionCode': { fontStyle: 'normal' },
        // Token colors aren't checked against the selected row's background.
        '.cm-tooltip-autocomplete > ul > li[aria-selected] .cm-btmux-completionCode *': { color: 'inherit' },
        '.cm-completionMatchedText': { textDecoration: 'none', color: theme.blue, fontWeight: 'bold' },
        ...completionIcons(theme),
        // Semantic tokens, in the roles `highlightSpecs` uses.
        '.cm-sem-namespace, .cm-sem-type, .cm-sem-class, .cm-sem-enum, .cm-sem-interface, .cm-sem-struct, .cm-sem-typeParameter':
          { color: theme.yellow },
        '.cm-sem-variable, .cm-sem-event': { color: theme.red },
        '.cm-sem-parameter, .cm-sem-property': { color: theme.foreground },
        '.cm-sem-enumMember, .cm-sem-number': { color: theme.brightRed },
        '.cm-sem-function, .cm-sem-method, .cm-sem-macro': { color: theme.blue },
        '.cm-sem-keyword, .cm-sem-modifier': { color: theme.magenta },
        '.cm-sem-comment': { color: theme.brightBlack },
        '.cm-sem-string': { color: theme.green },
        '.cm-sem-regexp, .cm-sem-operator': { color: theme.cyan },
        '.cm-sem-decorator': { color: theme.brightMagenta },
      },
      { dark: a.dark },
    ),
    EditorView.editorAttributes.of({
      style: [
        `--btm-editor-fg: ${theme.foreground}`,
        `--btm-editor-blue: ${theme.blue}`,
        `--btm-editor-green: ${theme.green}`,
        `--btm-editor-red: ${theme.red}`,
        `--btm-editor-yellow: ${theme.yellow}`,
        `--btm-editor-cyan: ${theme.cyan}`,
        `--btm-editor-selection: ${theme.selectionBackground}`,
      ].join('; '),
    }),
    syntaxHighlighting(HighlightStyle.define(highlightSpecs(theme))),
  ];
}
