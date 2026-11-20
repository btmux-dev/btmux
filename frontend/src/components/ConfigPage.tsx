import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';
import { Search } from 'lucide-react';
import { flushSync } from 'react-dom';
import { useToast } from '@astryxdesign/core/Toast';
import type { ClientMessage } from '../protocol/messages';
import type { ClientConfig, ColorMode } from '../state/types';
import { colorSchemeLabel, DEFAULT_COLOR_SCHEMES } from '../lib/colorSchemeLabel';
import { APPEARANCES } from '../lib/appearance';
import { systemColorMode } from '../state/startupTheme';
import { PANE_BORDER_STYLES } from '../lib/paneSwitchBorder';
import type { ShaderLayer } from '../generated/protocol';
import { hasShaderWallpaper, wallpaperLayersToml } from '../lib/wallpaperCatalog';
import { ShaderLayerStack } from './ShaderLayerStack';
import { useWallpaperCheck, type WallpaperStatus } from '../hooks/useWallpaperCheck';
import {
  getDesktopBackgroundOpacity,
  getFontWeightRange,
  getBackdropBlur,
  getBackdropDim,
  getPopupBlur,
  getPopupOpacity,
  getToolbarBlur,
  getToolbarOpacity,
  getToolbarSaturate,
  getPaneSwitchBorderSpeed,
  getShowPaneTitles,
  getTerminalFontFamily,
  getTerminalFontSize,
  getTerminalFontWeight,
  getWallpaperBlur,
  getWallpaperFollowsKeyboard,
  getWallpaperFollowsMouse,
  getWallpaperFps,
  getWallpaperOpacity,
  getWallpaperResolution,
  getWallpaperSaturate,
  getWallpaperSpeed,
} from '../state/configDefaults';
import { useStore } from '../state/store';
import { ConnectSettings, ConnectSetup } from './ConnectSettings';
import { Button } from '@astryxdesign/core/Button';
import { TextInput } from '@astryxdesign/core/TextInput';
import { NumberInput } from '@astryxdesign/core/NumberInput';
import { TextArea } from '@astryxdesign/core/TextArea';
import { Selector } from '@astryxdesign/core/Selector';
import { Switch } from '@astryxdesign/core/Switch';
import { Slider } from '@astryxdesign/core/Slider';
import { FormLayout } from '@astryxdesign/core/FormLayout';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { TabList, Tab } from '@astryxdesign/core/TabList';
import { cycleTabs, enterTabPanel } from '../lib/tabKeys';
import { Text } from '@astryxdesign/core/Text';
import { Heading } from '@astryxdesign/core/Heading';
import { VisuallyHidden } from '@astryxdesign/core/VisuallyHidden';
import { useClipboard, useMediaQuery } from '@astryxdesign/core/hooks';
import { Icon } from '@astryxdesign/core/Icon';
import { IconButton } from '@astryxdesign/core/IconButton';
import {
  Layout,
  LayoutContent,
  LayoutFooter,
  LayoutHeader,
  LayoutPanel,
  HStack,
  VStack,
} from '@astryxdesign/core/Layout';
import { KeyCap } from './KeyHint';
import { NARROW_DIALOG, NARROW_QUERY, TOUCH_QUERY, plainTextInput } from '../lib/touch';
import { actionLabel, popupIndex } from '../lib/actionLabel';
import { prefixError } from '../lib/prefixKey';

interface Props {
  config: ClientConfig;
  send: (message: ClientMessage) => void;
  /** Resolves with the command's error, or null. */
  request: (message: ClientMessage) => Promise<string | null>;
}

type Draft = {
  prefix: string;
  shell: string;
  appearance: ClientConfig['appearance'];
  colorsDark: string;
  colorsLight: string;
  fontFamily: string;
  fontWeight: number;
  fontSize: number;
  animations: boolean;
  showPaneTitles: boolean;
  sessionSort: ClientConfig['session_sort'];
  windowSort: ClientConfig['window_sort'];
  editor: ClientConfig['editor'];
  vimMode: boolean;
  windowGridCount: number;
  binds: Record<string, string>;
  keyOverrides: Record<string, string>;
  cursorBlink: boolean;
  scrollback: number;
  allowTransparency: boolean;
  disableStdin: boolean;
  macosEditKeys: boolean;
  smoothScrollDuration: number;
  cursorAnimationDuration: number;
  cellBackgroundOpacity: number;
  consoleLevel: string;
  fileLevel: string;
  osNotifications: boolean;
  osNotificationLevel: ClientConfig['notifications']['os-level'];
  wallpaper: string;
  wallpaperShaders: ShaderLayer[];
  wallpaperOpacity: number;
  desktopBackgroundOpacity: number;
  wallpaperBlur: number;
  wallpaperSaturate: number;
  wallpaperSpeed: number;
  wallpaperFps: number;
  wallpaperResolution: number;
  wallpaperFollowsMouse: boolean;
  wallpaperFollowsKeyboard: boolean;
  backdropBlur: number;
  backdropDim: number;
  popupOpacity: number;
  popupBlur: number;
  toolbarOpacity: number;
  toolbarBlur: number;
  toolbarSaturate: number;
  paneSwitchBorderStyle: string;
  paneSwitchBorderSpeed: number;
};

type DraftKey = keyof Draft;

// Shell choices that aren't commands.
const SHELL_DEFAULT = ':default';
const SHELL_CUSTOM = ':custom';
const FONT_CUSTOM = ':custom';
// Typed font names preview after a pause: each change rebuilds terminals.
const FONT_PREVIEW_DEBOUNCE_MS = 500;
type ConfigUpdate = Extract<ClientMessage, { type: 'update_config' }>['update'];

function initialDraft(config: ClientConfig): Draft {
  return {
    prefix: config.prefix,
    shell: config.shell ?? '',
    appearance: config.appearance,
    colorsDark: config.colors_dark ?? '',
    colorsLight: config.colors_light ?? '',
    fontFamily: getTerminalFontFamily(config),
    fontWeight: getTerminalFontWeight(config),
    fontSize: getTerminalFontSize(config),
    animations: config.animations,
    showPaneTitles: getShowPaneTitles(config),
    sessionSort: config.session_sort,
    windowSort: config.window_sort,
    editor: config.editor,
    vimMode: config.vim_mode,
    windowGridCount: config.window_grid_count,
    binds: Object.fromEntries(config.binds.map((bind) => [bind.action, bind.key])),
    keyOverrides: config.keys,
    cursorBlink: config.terminal.cursorBlink ?? true,
    scrollback: config.terminal.scrollback ?? 100_000,
    allowTransparency:
      config.terminal.allowTransparency ?? (config.wallpaper != null || hasShaderWallpaper(config.wallpaper_shaders)),
    disableStdin: config.terminal.disableStdin ?? false,
    macosEditKeys: config.terminal.macosEditKeys ?? true,
    smoothScrollDuration: config.terminal.smoothScrollDuration ?? 120,
    cursorAnimationDuration: config.terminal.cursorAnimationDuration ?? 120,
    cellBackgroundOpacity: config.terminal.cellBackgroundOpacity ?? 1,
    consoleLevel: config.log['console-level'],
    fileLevel: config.log['file-level'],
    osNotifications: config.notifications.os,
    osNotificationLevel: config.notifications['os-level'],
    wallpaper: config.wallpaper ?? '',
    wallpaperShaders: config.wallpaper_shaders,
    wallpaperOpacity: getWallpaperOpacity(config),
    desktopBackgroundOpacity: getDesktopBackgroundOpacity(config),
    wallpaperBlur: getWallpaperBlur(config),
    wallpaperSaturate: getWallpaperSaturate(config),
    wallpaperSpeed: getWallpaperSpeed(config),
    wallpaperFps: getWallpaperFps(config),
    wallpaperResolution: getWallpaperResolution(config),
    wallpaperFollowsMouse: getWallpaperFollowsMouse(config),
    wallpaperFollowsKeyboard: getWallpaperFollowsKeyboard(config),
    backdropBlur: getBackdropBlur(config),
    backdropDim: getBackdropDim(config),
    popupOpacity: getPopupOpacity(config),
    popupBlur: getPopupBlur(config),
    toolbarOpacity: getToolbarOpacity(config),
    toolbarBlur: getToolbarBlur(config),
    toolbarSaturate: getToolbarSaturate(config),
    paneSwitchBorderStyle: config.pane_switch_border ?? 'none',
    paneSwitchBorderSpeed: getPaneSwitchBorderSpeed(config),
  };
}

/** Fixed-width slider value, so every track in the form has the same length. */
function formatSliderValue(value: number, step: number): string {
  const decimals = Math.min(2, step < 1 ? (String(step).split('.')[1]?.length ?? 0) : 0);
  // Figure spaces are digit-wide and don't collapse.
  return value.toFixed(decimals).padStart(5, '\u2007');
}

function quote(value: string): string {
  return JSON.stringify(value);
}

// Mirrors the built-in defaults in `src/config.rs` (the `DEFAULT_*` constants
// and each struct's `Default` impl). Kept separate from `CONFIG_DEFAULTS`
// (frontend fallbacks for a not-yet-loaded config) since a couple of fields
// here have no wire-level equivalent (log levels).
const TOML_DEFAULTS = {
  prefix: 'C-a',
  animations: true,
  showPaneTitles: false,
  sessionSort: 'mru',
  windowSort: 'alphabetical',
  editor: 'native',
  vimMode: false,
  windowGridCount: 4,
  appearance: 'auto',
  wallpaperOpacity: 0.1,
  desktopBackgroundOpacity: 0.95,
  wallpaperBlur: 0,
  wallpaperSaturate: 0.05,
  wallpaperSpeed: 0.2,
  wallpaperFps: 30,
  wallpaperResolution: 0.4,
  wallpaperFollowsMouse: true,
  wallpaperFollowsKeyboard: false,
  backdropBlur: 2,
  backdropDim: 0.5,
  popupOpacity: 0.85,
  popupBlur: 2,
  toolbarOpacity: 0,
  toolbarBlur: 40,
  toolbarSaturate: 1,
  paneSwitchBorderStyle: 'wipe',
  paneSwitchBorderSpeed: 0.15,
  cursorBlink: true,
  scrollback: 100_000,
  fontFamily: 'Geist Mono',
  fontSize: 18,
  fontWeight: 300,
  disableStdin: false,
  macosEditKeys: true,
  smoothScrollDuration: 120,
  cursorAnimationDuration: 120,
  cellBackgroundOpacity: 1,
  consoleLevel: 'warn',
  fileLevel: 'info',
  osNotifications: true,
  osNotificationLevel: 'attention',
} as const;

function optLine(name: string, value: string): string | null {
  return value ? `${name} = ${quote(value)}` : null;
}

function strLine(name: string, value: string, fallback: string): string | null {
  return value === fallback ? null : `${name} = ${quote(value)}`;
}

function boolLine(name: string, value: boolean, fallback: boolean): string | null {
  return value === fallback ? null : `${name} = ${value}`;
}

function intLine(name: string, value: number, fallback: number): string | null {
  return value === fallback ? null : `${name} = ${value}`;
}

function numLine(name: string, value: number, fallback: number, decimals: number): string | null {
  const formatted = value.toFixed(decimals);
  return formatted === fallback.toFixed(decimals) ? null : `${name} = ${formatted}`;
}

function toToml(draft: Draft): string {
  const keyLines = Object.entries(draft.keyOverrides)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([action, key]) => `${action} = ${quote(key)}`);

  // `allow-transparency` has no fixed default: an unset value inherits
  // whichever behavior a wallpaper being present would already imply.
  const defaultAllowTransparency = draft.wallpaper !== '' || hasShaderWallpaper(draft.wallpaperShaders);

  const topLines = [
    strLine('prefix', draft.prefix, TOML_DEFAULTS.prefix),
    draft.shell ? `shell = ${quote(draft.shell)}` : null,
    boolLine('animations', draft.animations, TOML_DEFAULTS.animations),
    boolLine('show-pane-titles', draft.showPaneTitles, TOML_DEFAULTS.showPaneTitles),
    strLine('session-sort', draft.sessionSort, TOML_DEFAULTS.sessionSort),
    strLine('window-sort', draft.windowSort, TOML_DEFAULTS.windowSort),
    strLine('editor', draft.editor, TOML_DEFAULTS.editor),
    intLine('window-grid-count', draft.windowGridCount, TOML_DEFAULTS.windowGridCount),
    strLine('appearance', draft.appearance, TOML_DEFAULTS.appearance),
    optLine('colors-dark', draft.colorsDark),
    optLine('colors-light', draft.colorsLight),
    optLine('wallpaper', draft.wallpaper),
    numLine('wallpaper-opacity', draft.wallpaperOpacity, TOML_DEFAULTS.wallpaperOpacity, 2),
    numLine('desktop-background-opacity', draft.desktopBackgroundOpacity, TOML_DEFAULTS.desktopBackgroundOpacity, 2),
    numLine('wallpaper-blur', draft.wallpaperBlur, TOML_DEFAULTS.wallpaperBlur, 1),
    numLine('wallpaper-saturate', draft.wallpaperSaturate, TOML_DEFAULTS.wallpaperSaturate, 2),
    numLine('wallpaper-speed', draft.wallpaperSpeed, TOML_DEFAULTS.wallpaperSpeed, 2),
    intLine('wallpaper-fps', draft.wallpaperFps, TOML_DEFAULTS.wallpaperFps),
    numLine('wallpaper-resolution', draft.wallpaperResolution, TOML_DEFAULTS.wallpaperResolution, 2),
    boolLine('wallpaper-shader-follows-mouse-cursor', draft.wallpaperFollowsMouse, TOML_DEFAULTS.wallpaperFollowsMouse),
    boolLine(
      'wallpaper-shader-follows-keyboard-input',
      draft.wallpaperFollowsKeyboard,
      TOML_DEFAULTS.wallpaperFollowsKeyboard,
    ),
    numLine('backdrop-blur', draft.backdropBlur, TOML_DEFAULTS.backdropBlur, 1),
    numLine('backdrop-dim', draft.backdropDim, TOML_DEFAULTS.backdropDim, 2),
    numLine('popup-opacity', draft.popupOpacity, TOML_DEFAULTS.popupOpacity, 2),
    numLine('popup-blur', draft.popupBlur, TOML_DEFAULTS.popupBlur, 1),
    numLine('toolbar-opacity', draft.toolbarOpacity, TOML_DEFAULTS.toolbarOpacity, 2),
    numLine('toolbar-blur', draft.toolbarBlur, TOML_DEFAULTS.toolbarBlur, 1),
    numLine('toolbar-saturate', draft.toolbarSaturate, TOML_DEFAULTS.toolbarSaturate, 2),
    strLine('pane-switch-border', draft.paneSwitchBorderStyle, TOML_DEFAULTS.paneSwitchBorderStyle),
    numLine('pane-switch-border-speed', draft.paneSwitchBorderSpeed, TOML_DEFAULTS.paneSwitchBorderSpeed, 2),
  ].filter((line): line is string => line !== null);

  const terminalLines = [
    boolLine('cursor-blink', draft.cursorBlink, TOML_DEFAULTS.cursorBlink),
    intLine('scrollback', draft.scrollback, TOML_DEFAULTS.scrollback),
    strLine('font-family', draft.fontFamily.trim(), TOML_DEFAULTS.fontFamily),
    numLine('font-size', draft.fontSize, TOML_DEFAULTS.fontSize, 1),
    intLine('font-weight', draft.fontWeight, TOML_DEFAULTS.fontWeight),
    boolLine('allow-transparency', draft.allowTransparency, defaultAllowTransparency),
    boolLine('disable-stdin', draft.disableStdin, TOML_DEFAULTS.disableStdin),
    boolLine('macos-edit-keys', draft.macosEditKeys, TOML_DEFAULTS.macosEditKeys),
    numLine('smooth-scroll-duration', draft.smoothScrollDuration, TOML_DEFAULTS.smoothScrollDuration, 1),
    numLine('cursor-animation-duration', draft.cursorAnimationDuration, TOML_DEFAULTS.cursorAnimationDuration, 1),
    numLine('cell-background-opacity', draft.cellBackgroundOpacity, TOML_DEFAULTS.cellBackgroundOpacity, 2),
  ].filter((line): line is string => line !== null);

  const logLines = [
    strLine('console-level', draft.consoleLevel, TOML_DEFAULTS.consoleLevel),
    strLine('file-level', draft.fileLevel, TOML_DEFAULTS.fileLevel),
  ].filter((line): line is string => line !== null);

  const nativeEditorLines = [boolLine('vim', draft.vimMode, TOML_DEFAULTS.vimMode)].filter(
    (line): line is string => line !== null,
  );

  const notificationLines = [
    boolLine('os', draft.osNotifications, TOML_DEFAULTS.osNotifications),
    strLine('os-level', draft.osNotificationLevel, TOML_DEFAULTS.osNotificationLevel),
  ].filter((line): line is string => line !== null);

  const sections = [
    ['# Settings generated by btmux', ...topLines],
    wallpaperLayersToml(draft.wallpaperShaders),
    keyLines.length ? ['[keys]', ...keyLines] : [],
    terminalLines.length ? ['[terminal]', ...terminalLines] : [],
    logLines.length ? ['[log]', ...logLines] : [],
    notificationLines.length ? ['[notifications]', ...notificationLines] : [],
    nativeEditorLines.length ? ['[native-editor]', ...nativeEditorLines] : [],
  ].filter((section) => section.length > 0);

  return sections.map((section) => section.join('\n')).join('\n\n');
}

function toConfigUpdate(draft: Draft, dirty: Set<DraftKey>): ConfigUpdate {
  const update: ConfigUpdate = {};
  if (dirty.has('prefix')) update.prefix = draft.prefix;
  if (dirty.has('shell')) update.shell = draft.shell;
  if (dirty.has('showPaneTitles')) update.show_pane_titles = draft.showPaneTitles;
  if (dirty.has('sessionSort')) update.session_sort = draft.sessionSort;
  if (dirty.has('windowSort')) update.window_sort = draft.windowSort;
  if (dirty.has('editor')) update.editor = draft.editor;
  if (dirty.has('vimMode')) update.vim_mode = draft.vimMode;
  if (dirty.has('windowGridCount')) update.window_grid_count = draft.windowGridCount;
  if (dirty.has('binds')) update.keys = draft.keyOverrides;
  if (dirty.has('appearance')) update.appearance = draft.appearance;
  if (dirty.has('colorsDark')) update.colors_dark = draft.colorsDark;
  if (dirty.has('colorsLight')) update.colors_light = draft.colorsLight;
  if (dirty.has('fontFamily')) update.font_family = draft.fontFamily.trim();
  if (dirty.has('fontWeight')) update.font_weight = draft.fontWeight;
  if (dirty.has('fontSize')) update.font_size = draft.fontSize;
  if (dirty.has('cursorBlink')) update.cursor_blink = draft.cursorBlink;
  if (dirty.has('scrollback')) update.scrollback = draft.scrollback;
  if (dirty.has('allowTransparency')) update.allow_transparency = draft.allowTransparency;
  if (dirty.has('disableStdin')) update.disable_stdin = draft.disableStdin;
  if (dirty.has('macosEditKeys')) update.macos_edit_keys = draft.macosEditKeys;
  if (dirty.has('smoothScrollDuration')) update.smooth_scroll_duration = draft.smoothScrollDuration;
  if (dirty.has('cursorAnimationDuration')) update.cursor_animation_duration = draft.cursorAnimationDuration;
  if (dirty.has('cellBackgroundOpacity')) update.cell_background_opacity = draft.cellBackgroundOpacity;
  if (dirty.has('animations')) update.animations = draft.animations;
  if (dirty.has('consoleLevel')) update.console_level = draft.consoleLevel;
  if (dirty.has('fileLevel')) update.file_level = draft.fileLevel;
  if (dirty.has('osNotifications')) update.os_notifications = draft.osNotifications;
  if (dirty.has('osNotificationLevel')) update.os_notification_level = draft.osNotificationLevel;
  if (dirty.has('wallpaper')) update.wallpaper = draft.wallpaper;
  if (dirty.has('wallpaperShaders')) update.wallpaper_shaders = draft.wallpaperShaders;
  if (dirty.has('wallpaperOpacity')) update.wallpaper_opacity = draft.wallpaperOpacity;
  if (dirty.has('desktopBackgroundOpacity')) update.desktop_background_opacity = draft.desktopBackgroundOpacity;
  if (dirty.has('wallpaperBlur')) update.wallpaper_blur = draft.wallpaperBlur;
  if (dirty.has('wallpaperSaturate')) update.wallpaper_saturate = draft.wallpaperSaturate;
  if (dirty.has('wallpaperSpeed')) update.wallpaper_speed = draft.wallpaperSpeed;
  if (dirty.has('wallpaperFps')) update.wallpaper_fps = draft.wallpaperFps;
  if (dirty.has('wallpaperResolution')) update.wallpaper_resolution = draft.wallpaperResolution;
  if (dirty.has('wallpaperFollowsMouse')) {
    update.wallpaper_shader_follows_mouse_cursor = draft.wallpaperFollowsMouse;
  }
  if (dirty.has('wallpaperFollowsKeyboard')) {
    update.wallpaper_shader_follows_keyboard_input = draft.wallpaperFollowsKeyboard;
  }
  if (dirty.has('backdropBlur')) update.backdrop_blur = draft.backdropBlur;
  if (dirty.has('backdropDim')) update.backdrop_dim = draft.backdropDim;
  if (dirty.has('popupOpacity')) update.popup_opacity = draft.popupOpacity;
  if (dirty.has('popupBlur')) update.popup_blur = draft.popupBlur;
  if (dirty.has('toolbarOpacity')) update.toolbar_opacity = draft.toolbarOpacity;
  if (dirty.has('toolbarBlur')) update.toolbar_blur = draft.toolbarBlur;
  if (dirty.has('toolbarSaturate')) update.toolbar_saturate = draft.toolbarSaturate;
  if (dirty.has('paneSwitchBorderStyle')) update.pane_switch_border = draft.paneSwitchBorderStyle;
  if (dirty.has('paneSwitchBorderSpeed')) update.pane_switch_border_speed = draft.paneSwitchBorderSpeed;
  return update;
}

const COLORS_KEY = { dark: 'colorsDark', light: 'colorsLight' } as const satisfies Record<ColorMode, DraftKey>;

function previewTheme(config: ClientConfig, draft: Draft, dirty: Set<DraftKey>) {
  const mode = draft.appearance === 'auto' ? systemColorMode() : draft.appearance;
  const resolved = mode === 'dark' ? config.dark_theme : config.light_theme;
  const key = COLORS_KEY[mode];
  if (!dirty.has(key)) return resolved;
  return config.color_scheme_themes[draft[key] || DEFAULT_COLOR_SCHEMES[mode]] ?? resolved;
}

const TAB_LABELS = {
  general: 'General',
  notifications: 'Notifications',
  keybinds: 'Key binds',
  logging: 'Logging',
  wallpaper: 'Wallpaper',
  terminal: 'Terminal',
  effects: 'Effects',
  connect: 'Connect',
};
type SettingsTab = keyof typeof TAB_LABELS;
const SETTINGS_TABS = Object.keys(TAB_LABELS) as SettingsTab[];

/**
 * One setting: a stable id (its React key, so edits never remount it), the
 * words the search matches (with its tab's label and its config option
 * name), and its control. Runs of `form` entries share a FormLayout, `list`
 * ones a list.
 */
type Entry = { id: string; terms: string; node: ReactNode; kind: 'form' | 'list' | 'plain' };
const formEntry = (id: string, terms: string, node: ReactNode): Entry => ({ id, terms, node, kind: 'form' });
const listEntry = (id: string, terms: string, node: ReactNode): Entry => ({ id, terms, node, kind: 'list' });
const plainEntry = (id: string, terms: string, node: ReactNode): Entry => ({ id, terms, node, kind: 'plain' });
const isEntry = (entry: Entry | false): entry is Entry => entry !== false;
const withTerms = (entry: Entry, terms: string): Entry => ({ ...entry, terms: `${entry.terms} ${terms}` });
const TOOLBAR_TERMS = 'side bar sidebar status bar glass transparency';

/** Config file names of the draft's fields, so a key pasted from config.toml or the docs finds its setting. */
const OPTION_NAMES: Record<DraftKey, string> = {
  prefix: 'prefix',
  shell: 'shell',
  appearance: 'appearance',
  colorsDark: 'colors-dark theme-dark',
  colorsLight: 'colors-light theme-light',
  fontFamily: 'font-family',
  fontWeight: 'font-weight',
  fontSize: 'font-size',
  animations: 'animations',
  showPaneTitles: 'show-pane-titles',
  sessionSort: 'session-sort',
  windowSort: 'window-sort',
  editor: 'editor',
  vimMode: 'vim native-editor',
  windowGridCount: 'window-grid-count',
  binds: 'keys',
  keyOverrides: 'keys',
  cursorBlink: 'cursor-blink',
  scrollback: 'scrollback',
  allowTransparency: 'allow-transparency',
  disableStdin: 'disable-stdin',
  macosEditKeys: 'macos-edit-keys',
  smoothScrollDuration: 'smooth-scroll-duration',
  cursorAnimationDuration: 'cursor-animation-duration',
  cellBackgroundOpacity: 'cell-background-opacity',
  consoleLevel: 'console-level',
  fileLevel: 'file-level',
  osNotifications: 'os notifications',
  osNotificationLevel: 'os-level',
  wallpaper: 'wallpaper',
  wallpaperShaders: 'wallpaper-shader',
  wallpaperOpacity: 'wallpaper-opacity',
  desktopBackgroundOpacity: 'desktop-background-opacity',
  wallpaperBlur: 'wallpaper-blur',
  wallpaperSaturate: 'wallpaper-saturate',
  wallpaperSpeed: 'wallpaper-speed',
  wallpaperFps: 'wallpaper-fps',
  wallpaperResolution: 'wallpaper-resolution',
  wallpaperFollowsMouse: 'wallpaper-shader-follows-mouse-cursor',
  wallpaperFollowsKeyboard: 'wallpaper-shader-follows-keyboard-input',
  backdropBlur: 'backdrop-blur',
  backdropDim: 'backdrop-dim',
  popupOpacity: 'popup-opacity',
  popupBlur: 'popup-blur',
  toolbarOpacity: 'toolbar-opacity',
  toolbarBlur: 'toolbar-blur',
  toolbarSaturate: 'toolbar-saturate',
  paneSwitchBorderStyle: 'pane-switch-border',
  paneSwitchBorderSpeed: 'pane-switch-border-speed',
};

/** Whether every word of the query is in the entry's tab label, terms, or config option name. */
function matchesQuery(tabLabel: string, entry: Entry, words: string[]): boolean {
  const option = (OPTION_NAMES as Record<string, string | undefined>)[entry.id] ?? '';
  const terms = `${tabLabel} ${entry.terms} ${option}`.toLowerCase();
  return words.every((word) => terms.includes(word));
}

function renderEntries(entries: Entry[]): ReactNode[] {
  const runs: Entry[][] = [];
  for (const entry of entries) {
    const last = runs[runs.length - 1];
    if (last && last[0].kind === entry.kind && entry.kind !== 'plain') last.push(entry);
    else runs.push([entry]);
  }
  return runs.map((run, index) => {
    const key = `${run[0].kind}-${index}`;
    const nodes = run.map((entry) => <Fragment key={entry.id}>{entry.node}</Fragment>);
    if (run[0].kind === 'form') return <FormLayout key={key}>{nodes}</FormLayout>;
    if (run[0].kind === 'list')
      return (
        <VStack key={key} gap={1}>
          {nodes}
        </VStack>
      );
    return <Fragment key={key}>{nodes}</Fragment>;
  });
}

const CONTROLS = 'a[href], button, input, select, textarea, [tabindex]:not([tabindex="-1"])';

function firstControl(root: HTMLElement | null): HTMLElement | undefined {
  return [...(root?.querySelectorAll<HTMLElement>(CONTROLS) ?? [])].find(
    (item) => !item.matches(':disabled') && item.checkVisibility(),
  );
}

/**
 * The first Escape during a search clears it, wherever focus is in the
 * dialog, rather than closing Settings and dropping its edits. An open
 * popup (a selector's options) and key recording, which listens first,
 * keep theirs.
 */
function escapeClearsSearch(e: ReactKeyboardEvent<HTMLElement>, searching: boolean): boolean {
  if (e.key !== 'Escape' || !searching || e.defaultPrevented) return false;
  const target = e.target as Element;
  if (target.closest('[aria-haspopup][aria-expanded="true"], [role="listbox"], [role="menu"]')) return false;
  e.preventDefault();
  e.stopPropagation();
  return true;
}

/**
 * A character typed outside a text field (on a tab, a switch, the dialog)
 * goes to the search field, which takes focus before the key types into it.
 */
function typeToSearch(e: ReactKeyboardEvent<HTMLElement>, search: HTMLInputElement | null): boolean {
  if (!search || e.key.length !== 1 || e.key === ' ' || e.ctrlKey || e.metaKey || e.altKey) return false;
  const target = e.target as HTMLElement;
  if (target === search || !e.currentTarget.contains(target)) return false;
  if (target.matches('input, textarea, select, [contenteditable]')) return false;
  // Listbox and menu typeahead.
  if (target.closest('[role="listbox"], [role="menu"], [role="option"], [role="menuitem"]')) return false;
  search.focus();
  return true;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'CapsLock']);

/**
 * A binding shown as its key cap; click it, then press the new key. Escape
 * cancels. Touch screens type the key into an input, which opens the keyboard.
 */
function KeyBindField({ label, value, onChange }: { label: string; value: string; onChange: (key: string) => void }) {
  const [recording, setRecording] = useState(false);
  const touch = useMediaQuery(TOUCH_QUERY);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!recording) return;
    // Window capture runs before the keybinding hook and the dialog's Escape handling.
    const onKeyDown = (e: KeyboardEvent) => {
      // Android keyboards send Unidentified; the touch input reads their text.
      if (MODIFIER_KEYS.has(e.key) || e.key === 'Unidentified' || e.isComposing) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      if (e.key !== 'Escape') onChange(e.key);
      setRecording(false);
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [recording, onChange]);

  if (touch && recording)
    return (
      <TextInput
        ref={(input) => {
          inputRef.current = input;
          plainTextInput(input);
        }}
        label={`Type a key for ${label}`}
        isLabelHidden
        size="sm"
        value=""
        placeholder={`${label}: type a key…`}
        autoComplete="off"
        onChange={(typed) => {
          const key = [...typed].pop();
          if (key) onChange(key);
          setRecording(false);
        }}
        onBlur={() => setRecording(false)}
      />
    );

  return (
    <Button
      label={recording ? `Press a key for ${label}` : `Change key for ${label}`}
      size="sm"
      variant="ghost"
      width="100%"
      onClick={() => {
        if (!touch) return setRecording((current) => !current);
        // iOS opens the keyboard only for focus taken during the tap.
        flushSync(() => setRecording(true));
        inputRef.current?.focus();
      }}
      onBlur={() => setRecording(false)}
      className="[&>span>span]:flex-1"
    >
      <HStack as="span" gap={3} vAlign="center" hAlign="between" className="w-full">
        <Text>{label}</Text>
        {recording ? (
          <Text color="accent">Press a key…</Text>
        ) : value ? (
          <KeyCap keys={value} />
        ) : (
          <Text color="secondary">Unbound</Text>
        )}
      </HStack>
    </Button>
  );
}

export function ConfigPage({ config, send, request }: Props) {
  const isNarrow = useMediaQuery(NARROW_QUERY);
  const [tab, setTab] = useState<SettingsTab>('general');
  const connect = useStore((state) => state.connect);
  const setConfigPreview = useStore((state) => state.setConfigPreview);
  const setSettingsOpen = useStore((state) => state.setSettingsOpen);
  const toast = useToast();
  const [draft, setDraft] = useState(() => initialDraft(config));
  const [customShell, setCustomShell] = useState(false);
  const [customFont, setCustomFont] = useState(false);
  const [typedFont, setTypedFont] = useState(draft.fontFamily);
  const wallpaperCheck = useWallpaperCheck(draft.wallpaper);
  const [dirty, setDirty] = useState<Set<DraftKey>>(() => new Set());
  const { copy, isCopied } = useClipboard({ announce: 'Copied' });
  const [showToml, setShowToml] = useState(false);
  const [query, setQuery] = useState('');
  const searchRef = useRef<HTMLInputElement | null>(null);
  const resultsRef = useRef<HTMLElement>(null);
  const toml = useMemo(() => toToml(draft), [draft]);

  const knownFont = config.fonts.some((font) => font.family === draft.fontFamily);
  const fontChoice = customFont || !knownFont ? FONT_CUSTOM : draft.fontFamily;
  const fontInvalid =
    fontChoice === FONT_CUSTOM && draft.fontFamily.trim() === '' ? 'Enter the name of an installed font.' : null;
  useEffect(() => {
    const timer = setTimeout(() => setTypedFont(draft.fontFamily.trim()), FONT_PREVIEW_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [draft.fontFamily]);
  const previewFont = fontChoice === FONT_CUSTOM ? typedFont || getTerminalFontFamily(config) : draft.fontFamily;

  const previewConfig = useMemo<ClientConfig>(
    () => ({
      ...config,
      theme: previewTheme(config, draft, dirty),
      animations: draft.animations,
      show_pane_titles: draft.showPaneTitles,
      vim_mode: draft.vimMode,
      terminal: {
        ...config.terminal,
        cursorBlink: draft.cursorBlink,
        scrollback: draft.scrollback,
        fontFamily: previewFont,
        fontSize: draft.fontSize,
        fontWeight: draft.fontWeight,
        // Keep null until the user changes this control so a wallpaper can
        // still enable terminal transparency through its normal default.
        allowTransparency: dirty.has('allowTransparency') ? draft.allowTransparency : config.terminal.allowTransparency,
        disableStdin: dirty.has('disableStdin') ? draft.disableStdin : config.terminal.disableStdin,
        macosEditKeys: dirty.has('macosEditKeys') ? draft.macosEditKeys : config.terminal.macosEditKeys,
        smoothScrollDuration: dirty.has('smoothScrollDuration')
          ? draft.smoothScrollDuration
          : config.terminal.smoothScrollDuration,
        cursorAnimationDuration: dirty.has('cursorAnimationDuration')
          ? draft.cursorAnimationDuration
          : config.terminal.cursorAnimationDuration,
        cellBackgroundOpacity: draft.cellBackgroundOpacity,
      },
      wallpaper: draft.wallpaper || null,
      // An edited wallpaper previews once checked, through the same proxy as applied ones.
      wallpaper_url: draft.wallpaper === (config.wallpaper ?? '') ? config.wallpaper_url : wallpaperCheck.url,
      wallpaper_shaders: draft.wallpaperShaders,
      wallpaper_opacity: draft.wallpaperOpacity,
      desktop_background_opacity: draft.desktopBackgroundOpacity,
      wallpaper_blur: draft.wallpaperBlur,
      wallpaper_saturate: draft.wallpaperSaturate,
      wallpaper_speed: draft.wallpaperSpeed,
      wallpaper_fps: draft.wallpaperFps,
      wallpaper_resolution: draft.wallpaperResolution,
      wallpaper_shader_follows_mouse_cursor: draft.wallpaperFollowsMouse,
      wallpaper_shader_follows_keyboard_input: draft.wallpaperFollowsKeyboard,
      backdrop_blur: draft.backdropBlur,
      backdrop_dim: draft.backdropDim,
      popup_opacity: draft.popupOpacity,
      popup_blur: draft.popupBlur,
      toolbar_opacity: draft.toolbarOpacity,
      toolbar_blur: draft.toolbarBlur,
      toolbar_saturate: draft.toolbarSaturate,
      pane_switch_border: draft.paneSwitchBorderStyle || null,
      pane_switch_border_speed: draft.paneSwitchBorderSpeed,
    }),
    [config, draft, dirty, wallpaperCheck.url, previewFont],
  );

  // The active session consumes this local layer directly, so draft changes
  // update its real terminals without a control-socket round trip.
  useEffect(() => {
    setConfigPreview(previewConfig);
  }, [previewConfig, setConfigPreview]);
  useEffect(() => () => setConfigPreview(null), [setConfigPreview]);

  // Config broadcasts are authoritative. This also refreshes the local draft
  // after reset, when the server replaces session-only overrides with the
  // values resolved from config.toml and built-in defaults.
  useEffect(() => {
    setDraft(initialDraft(config));
    setDirty(new Set());
    setCustomShell(false);
    setCustomFont(false);
  }, [config]);

  const update = <K extends DraftKey>(key: K, value: Draft[K]) => {
    setDraft((current) => ({ ...current, [key]: value }));
    setDirty((current) => new Set(current).add(key));
  };

  const knownShell = draft.shell === '' || config.shells.some((shell) => shell.command === draft.shell);
  const shellChoice = customShell || !knownShell ? SHELL_CUSTOM : draft.shell || SHELL_DEFAULT;
  const chooseShell = (value: string) => {
    setCustomShell(value === SHELL_CUSTOM);
    if (value !== SHELL_CUSTOM) update('shell', value === SHELL_DEFAULT ? '' : value);
  };

  const chooseFont = (value: string) => {
    setCustomFont(value === FONT_CUSTOM);
    if (value === FONT_CUSTOM) return;
    const { min, max } = getFontWeightRange(config.fonts, value);
    update('fontFamily', value);
    update('fontWeight', Math.min(max, Math.max(min, draft.fontWeight)));
  };

  const prefixInvalid = dirty.has('prefix') && draft.prefix.trim() !== '' ? prefixError(draft.prefix) : null;

  const apply = () => {
    if (prefixInvalid || fontInvalid) return;
    const update = toConfigUpdate(draft, dirty);
    if (Object.keys(update).length === 0) return;
    send({ type: 'update_config', update });
    setDirty(new Set());
    toast({ body: 'Applied until restart' });
    setSettingsOpen(false);
  };

  const [saving, setSaving] = useState(false);
  // Writes the applied overrides plus the draft to config.toml. A read-only
  // file (Nix, Home Manager) fails with a toast; the TOML is shown to copy.
  const save = async () => {
    if (prefixInvalid || fontInvalid) return;
    setSaving(true);
    const error = await request({ type: 'save_config', update: toConfigUpdate(draft, dirty) });
    setSaving(false);
    if (error) setShowToml(true);
    else setSettingsOpen(false);
  };

  const reset = () => {
    send({ type: 'reset_config' });
    setDirty(new Set());
    toast({ body: 'Settings reset to config.toml and defaults' });
  };

  const { min: weightMin, max: weightMax } = getFontWeightRange(config.fonts, draft.fontFamily);

  // `[[popup]]` tables set popup keys, not `[keys]`.
  const bindingRows = [
    ...new Map(
      config.binds.filter((bind) => popupIndex(bind.action) === null).map((bind) => [bind.action, bind]),
    ).values(),
  ].sort((left, right) => left.action.localeCompare(right.action));

  const goBack = () => setSettingsOpen(false);
  const [osPermission, setOsPermission] = useState<NotificationPermission | 'unsupported'>(() =>
    typeof Notification === 'undefined' ? 'unsupported' : Notification.permission,
  );

  const toggle = (
    key: { [K in DraftKey]: Draft[K] extends boolean ? K : never }[DraftKey],
    label: string,
    description?: string,
  ): Entry =>
    formEntry(
      key,
      `${label} ${description ?? ''}`,
      <Switch
        key={key}
        label={label}
        description={description}
        value={draft[key]}
        onChange={(value) => update(key, value)}
        labelPosition="start"
        labelSpacing="spread"
      />,
    );
  const text = (
    key: { [K in DraftKey]: Draft[K] extends string ? K : never }[DraftKey],
    label: string,
    description?: string,
    /** An error message, or a full status. */
    status?: string | WallpaperStatus | null,
  ): Entry =>
    formEntry(
      key,
      `${label} ${description ?? ''}`,
      <TextInput
        key={key}
        ref={plainTextInput}
        label={label}
        value={draft[key]}
        onChange={(value) => update(key, value as never)}
        description={description}
        status={typeof status === 'string' ? { type: 'error', message: status } : (status ?? undefined)}
      />,
    );
  const range = (
    key: { [K in DraftKey]: Draft[K] extends number ? K : never }[DraftKey],
    label: string,
    min: number,
    max: number,
    step = 1,
    isDisabled = false,
  ): Entry =>
    formEntry(
      key,
      label,
      <Slider
        key={key}
        label={label}
        value={draft[key]}
        min={min}
        max={max}
        step={step}
        onChange={(value: number) => update(key, value)}
        valueDisplay="text"
        formatValue={(value) => formatSliderValue(value, step)}
        isDisabled={isDisabled}
      />,
    );
  const choose = (
    key: { [K in DraftKey]: Draft[K] extends string ? K : never }[DraftKey],
    label: string,
    options: { value: string; label: string; description?: string }[],
    description?: string,
  ): Entry =>
    formEntry(
      key,
      `${label} ${description ?? ''} ${options.map((option) => option.label).join(' ')}`,
      <Selector
        key={key}
        label={label}
        description={description}
        value={draft[key] || 'none'}
        options={options}
        onChange={(value) => update(key, (value === 'none' ? '' : value) as never)}
        presentation="adaptive"
      />,
    );
  const none = { value: 'none', label: 'None' };
  const ordering = [
    { value: 'created', label: 'Created' },
    { value: 'mru', label: 'Recently used' },
    { value: 'alphabetical', label: 'Alphabetical' },
  ];
  const paneBorderOff = !draft.paneSwitchBorderStyle || draft.paneSwitchBorderStyle === 'none';

  // Every tab's settings, which the search filters across tabs.
  const entries: Record<SettingsTab, Entry[]> = {
    general: [
      choose(
        'appearance',
        'Appearance',
        APPEARANCES.map(({ value, label }) => ({ value, label })),
        'Auto follows the system setting.',
      ),
      ...(['dark', 'light'] as const).map((mode) => {
        const label = mode === 'dark' ? 'Dark color scheme' : 'Light color scheme';
        return formEntry(
          COLORS_KEY[mode],
          `${label} theme palette`,
          <Selector
            key={mode}
            label={label}
            value={draft[COLORS_KEY[mode]] || DEFAULT_COLOR_SCHEMES[mode]}
            hasSearch
            presentation="adaptive"
            options={config.color_schemes.map((value) => ({
              value,
              label: colorSchemeLabel(value),
            }))}
            onChange={(value) => update(COLORS_KEY[mode], value)}
          />,
        );
      }),
      formEntry(
        'fontChoice',
        'Terminal font Also used for paths, names, and key hints in the UI. font-family',
        <Selector
          key="font"
          label="Terminal font"
          description="Also used for paths, names, and key hints in the UI."
          value={fontChoice}
          presentation="adaptive"
          options={[
            {
              type: 'section',
              title: 'Bundled',
              options: config.fonts.map((font) => ({ value: font.family, label: font.family })),
            },
            { type: 'section', title: 'Custom', options: [{ value: FONT_CUSTOM, label: 'Custom…' }] },
          ]}
          onChange={chooseFont}
        />,
      ),
      fontChoice === FONT_CUSTOM && text('fontFamily', 'Font family', 'A font installed on this device.', fontInvalid),
      text('prefix', 'Prefix key', 'Use tmux notation such as C-a, C-b, M-x, or C-Space.', prefixInvalid),
      formEntry(
        'shellChoice',
        'Shell for new panes shell',
        <Selector
          key="shell"
          label="Shell for new panes"
          value={config.cli_shell ?? shellChoice}
          presentation="adaptive"
          isDisabled={config.cli_shell != null}
          disabledMessage="Set by btmux --shell."
          options={
            config.cli_shell != null
              ? [{ value: config.cli_shell, label: config.cli_shell }]
              : [
                  { value: SHELL_DEFAULT, label: `Default (${config.default_shell})` },
                  ...config.shells.map((shell) => ({
                    value: shell.command,
                    label: shell.label,
                    description: shell.command,
                  })),
                  { type: 'divider' as const },
                  { value: SHELL_CUSTOM, label: 'Custom…' },
                ]
          }
          onChange={chooseShell}
        />,
      ),
      config.cli_shell == null &&
        shellChoice === SHELL_CUSTOM &&
        text('shell', 'Shell command', 'A program, with arguments on Windows (wsl.exe -d Ubuntu).'),
      toggle('animations', 'Animations'),
      toggle('showPaneTitles', 'Show pane titles', 'Title bar with command, directory, and size above each pane.'),
      choose('sessionSort', 'Session sort', ordering),
      choose('windowSort', 'Window sort', ordering),
      choose(
        'editor',
        'Editor',
        [
          {
            value: 'native',
            label: 'btmux',
            description: "btmux's own editor.",
          },
          {
            value: 'neovim',
            label: 'Neovim',
            description:
              "Run headless Neovim with btmux's built-in GUI. Injects config for theming and UI. (Requires Neovim 0.12+ on PATH.)",
          },
          {
            value: 'vscode',
            label: 'VS Code',
            description:
              'Run headless VS Code (code serve-web). Injects an extension to sync theme and font. (Requires VS Code.)',
          },
          {
            value: 'pane',
            label: 'Terminal',
            description: 'Run $EDITOR in the pane.',
          },
        ],
        'Opens with prefix + e, or from the file browser and Git mode.',
      ),
      toggle(
        'vimMode',
        'Vim mode',
        "Vim keybindings in btmux's editor. :w saves, :q hides it, and a double Esc hides it only from normal mode.",
      ),
      formEntry(
        'windowGridCount',
        'Window grid count',
        <NumberInput
          key="windowGridCount"
          label="Window grid count"
          value={draft.windowGridCount}
          min={1}
          max={24}
          isIntegerOnly
          onChange={(value) => update('windowGridCount', value)}
        />,
      ),
      range('desktopBackgroundOpacity', 'Desktop background opacity', 0, 1, 0.01),
    ].filter(isEntry),
    terminal: [
      range('fontSize', 'Terminal font size', 8, 36),
      range('fontWeight', 'Terminal font weight', weightMin, weightMax, 100),
      toggle('cursorBlink', 'Blinking cursor'),
      formEntry(
        'scrollback',
        'Scrollback lines history',
        <NumberInput
          key="scrollback"
          label="Scrollback lines"
          value={draft.scrollback}
          min={1}
          max={1_000_000}
          isIntegerOnly
          onChange={(value) => update('scrollback', value)}
        />,
      ),
      toggle('allowTransparency', 'Allow transparency'),
      toggle('disableStdin', 'Disable terminal input'),
      toggle(
        'macosEditKeys',
        'macOS Cmd editing keys',
        'Cmd+Left/Right, Cmd+Backspace, and Cmd+Delete send C-a, C-e, C-u, and C-k',
      ),
      range('smoothScrollDuration', 'Smooth scroll duration (ms)', 0, 500, 10),
      range('cursorAnimationDuration', 'Cursor animation duration (ms)', 0, 500, 10),
      range('cellBackgroundOpacity', 'Cell background opacity', 0, 1, 0.05),
    ],
    wallpaper: [
      text('wallpaper', 'Wallpaper URL or path', 'Drawn beneath the shader layers.', wallpaperCheck.status),
      range('wallpaperOpacity', 'Wallpaper opacity', 0, 1, 0.01),
      range('wallpaperBlur', 'Wallpaper blur', 0, 40, 0.5),
      range('wallpaperSaturate', 'Wallpaper saturation', 0, 3, 0.05),
      range('wallpaperSpeed', 'Wallpaper speed', 0, 10, 0.05),
      range('wallpaperFps', 'Wallpaper frame rate', 10, 60, 1),
      range('wallpaperResolution', 'Wallpaper resolution', 0.1, 1, 0.05),
      toggle('wallpaperFollowsMouse', 'Follow mouse cursor'),
      toggle(
        'wallpaperFollowsKeyboard',
        'Follow keyboard input',
        'Drives interactive effects from the active terminal cursor.',
      ),
      plainEntry(
        'wallpaperShaders',
        'Shader layers generators effects',
        <ShaderLayerStack
          key="shaders"
          layers={draft.wallpaperShaders}
          onChange={(layers) => update('wallpaperShaders', layers)}
        />,
      ),
    ],
    effects: [
      range('backdropBlur', 'Modal backdrop blur (px)', 0, 50, 0.5),
      range('backdropDim', 'Modal backdrop dimming', 0, 1, 0.05),
      range('popupOpacity', 'Editor popup opacity', 0, 1, 0.05),
      range('popupBlur', 'Editor popup blur (px)', 0, 50, 0.5),
      withTerms(range('toolbarOpacity', 'Toolbar opacity', 0, 1, 0.05), TOOLBAR_TERMS),
      withTerms(range('toolbarBlur', 'Toolbar blur (px)', 0, 80, 1), TOOLBAR_TERMS),
      withTerms(range('toolbarSaturate', 'Toolbar saturation', 0, 3, 0.05), TOOLBAR_TERMS),
      choose('paneSwitchBorderStyle', 'Pane switch border animation', [
        none,
        ...PANE_BORDER_STYLES.map((item) => ({ value: item.id, label: item.label })),
      ]),
      range('paneSwitchBorderSpeed', 'Border duration (s)', 0.05, 1, 0.01, paneBorderOff),
    ],
    keybinds: bindingRows.map((bind) => {
      const value = draft.binds[bind.action] ?? bind.key;
      return listEntry(
        `bind:${bind.action}`,
        `${actionLabel(bind.action)} ${bind.action} ${value}`,
        <KeyBindField
          key={bind.action}
          label={actionLabel(bind.action)}
          value={value}
          onChange={(key) => {
            setDraft((current) => ({
              ...current,
              binds: { ...current.binds, [bind.action]: key },
              keyOverrides: { ...current.keyOverrides, [bind.action]: key },
            }));
            setDirty((current) => new Set(current).add('binds'));
          }}
        />,
      );
    }),
    notifications: [
      toggle('osNotifications', 'OS notifications', 'Alert while btmux is hidden or unfocused.'),
      choose('osNotificationLevel', 'Minimum level', [
        { value: 'info', label: 'Info and above' },
        { value: 'success', label: 'Success and above' },
        { value: 'attention', label: 'Attention and error' },
        { value: 'error', label: 'Errors only' },
      ]),
      osPermission !== 'granted' &&
        plainEntry(
          'osPermission',
          'OS notifications browser permission allow',
          osPermission === 'default' ? (
            <HStack key="permission" gap={2} vAlign="center">
              <Text color="secondary">This browser has not allowed notifications yet.</Text>
              <Button label="Allow" onClick={() => void Notification.requestPermission().then(setOsPermission)} />
            </HStack>
          ) : osPermission === 'denied' ? (
            <Text key="permission" color="secondary">
              Blocked; allow in site settings.
            </Text>
          ) : (
            <Text key="permission" color="secondary">
              This browser does not support notifications.
            </Text>
          ),
        ),
    ].filter(isEntry),
    connect: [
      plainEntry(
        'connect',
        'Connect remote access relay passkeys sign in invite',
        connect ? <ConnectSettings key="connect" status={connect} /> : <ConnectSetup key="connect" />,
      ),
    ],
    logging: [
      plainEntry(
        'logIntro',
        'Log levels console file',
        <Text key="intro" color="secondary">
          Use error, warn, info, debug, trace, or a tracing directive. Applies after restart.
        </Text>,
      ),
      text('consoleLevel', 'Console level'),
      text('fileLevel', 'File level'),
    ],
  };

  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const searching = words.length > 0;
  const results = SETTINGS_TABS.map((value) => ({
    tab: value,
    found: entries[value].filter((item) => matchesQuery(TAB_LABELS[value], item, words)),
  })).filter(({ found }) => found.length > 0);
  const resultCount = results.reduce((sum, { found }) => sum + found.length, 0);

  const tomlPanel = (
    <LayoutPanel
      width={isNarrow ? '100%' : '50%'}
      padding={0}
      hasDivider={!isNarrow}
      isScrollable={false}
      role="region"
      label="Generated TOML"
      className={isNarrow ? 'h-64 border-t border-border' : 'h-full'}
    >
      <Layout
        content={
          <LayoutContent padding={6} isScrollable={false} className="relative [&>.astryx-field]:h-full">
            <TextArea
              label="Generated TOML"
              isLabelHidden
              value={toml}
              isReadOnly
              hasSpellCheck={false}
              statusVariant="detached"
              controlClassName="h-full [&_textarea]:h-full [&_textarea]:resize-none [&_textarea]:pr-10 [&_textarea]:font-mono"
            />
            <IconButton
              variant="ghost"
              size="sm"
              icon={<Icon icon={isCopied ? 'check' : 'copy'} size="sm" color="inherit" />}
              tooltip="Copy"
              label={isCopied ? 'Copied' : 'Copy TOML'}
              onClick={() => void copy(toml)}
              className="absolute top-8 right-8"
            />
          </LayoutContent>
        }
      />
    </LayoutPanel>
  );

  return (
    <Dialog
      isOpen
      onOpenChange={(open) => {
        if (!open) goBack();
      }}
      purpose="form"
      {...(isNarrow ? NARROW_DIALOG : { width: showToml ? 1200 : 640, maxHeight: '90dvh' })}
      // No blur or dim: the app behind previews the changes.
      className="h-full btm-clear-backdrop"
      // Clicking text keeps focus here, so Tab still reaches the handler.
      tabIndex={-1}
      onKeyDown={(e) => {
        if (escapeClearsSearch(e, searching)) {
          setQuery('');
          // The focused result may unmount with the results.
          searchRef.current?.focus();
          return;
        }
        if (typeToSearch(e, searchRef.current)) return;
        if (!searching && !cycleTabs(e, SETTINGS_TABS, tab, setTab)) enterTabPanel(e);
      }}
    >
      <Layout
        header={
          <DialogHeader
            title="Settings"
            hasDivider={false}
            onOpenChange={(open) => {
              if (!open) goBack();
            }}
          />
        }
        end={showToml && !isNarrow ? tomlPanel : undefined}
        footer={showToml && isNarrow ? tomlPanel : undefined}
        content={
          <Layout
            header={
              <LayoutHeader paddingBlockEnd={searching ? undefined : 0}>
                <VStack gap={3}>
                  <TextInput
                    ref={(input) => {
                      searchRef.current = input;
                      plainTextInput(input);
                    }}
                    label="Search settings"
                    isLabelHidden
                    placeholder="Search settings"
                    startIcon={Search}
                    hasClear
                    autoComplete="off"
                    value={query}
                    onChange={setQuery}
                    onKeyDown={(e) => {
                      if (e.key === 'ArrowDown' && searching) {
                        e.preventDefault();
                        firstControl(resultsRef.current)?.focus();
                      }
                    }}
                  />
                  {!searching && (
                    <TabList
                      value={tab}
                      onChange={(value) => setTab(value as SettingsTab)}
                      role="tablist"
                      hasDivider
                      isFullBleed
                    >
                      {SETTINGS_TABS.map((value) => (
                        <Tab key={value} value={value} label={TAB_LABELS[value]} panelId={`settings-${value}`} />
                      ))}
                    </TabList>
                  )}
                  {/* Mounted throughout, so screen readers announce changes from the search field. */}
                  <VisuallyHidden as="div" role="status" id="settings-search-status">
                    {!searching
                      ? ''
                      : resultCount
                        ? `${resultCount} ${resultCount === 1 ? 'setting' : 'settings'} found`
                        : `No settings match “${query.trim()}”.`}
                  </VisuallyHidden>
                </VStack>
              </LayoutHeader>
            }
            content={
              <LayoutContent padding={6}>
                {!searching ? (
                  <VStack gap={4} id={`settings-${tab}`} role="tabpanel" aria-label={TAB_LABELS[tab]}>
                    {renderEntries(entries[tab])}
                  </VStack>
                ) : results.length ? (
                  <VStack ref={resultsRef} gap={8} role="region" aria-label="Search results">
                    {results.map(({ tab: value, found }) => (
                      <VStack key={value} gap={4} role="group" aria-label={TAB_LABELS[value]}>
                        <Heading level={4}>{TAB_LABELS[value]}</Heading>
                        {renderEntries(found)}
                      </VStack>
                    ))}
                  </VStack>
                ) : (
                  <Text color="secondary">No settings match “{query.trim()}”.</Text>
                )}
              </LayoutContent>
            }
            footer={
              <LayoutFooter hasDivider>
                <HStack gap={2} wrap="wrap" hAlign="between" vAlign="center">
                  <HStack gap={4} wrap="wrap" vAlign="center">
                    <Button label="Reset" onClick={reset} />
                    <Switch label="Show TOML" value={showToml} onChange={setShowToml} />
                  </HStack>
                  <HStack gap={2} wrap="wrap" vAlign="center">
                    <Button label="Close" onClick={goBack} />
                    <Button
                      label="Apply"
                      onClick={apply}
                      isDisabled={dirty.size === 0 || !!prefixInvalid || !!fontInvalid}
                    />
                    <Button
                      label="Save"
                      variant="primary"
                      tooltip="Write to config.toml"
                      onClick={() => void save()}
                      isLoading={saving}
                      isDisabled={(dirty.size === 0 && !config.has_overrides) || !!prefixInvalid || !!fontInvalid}
                    />
                  </HStack>
                </HStack>
              </LayoutFooter>
            }
          />
        }
      />
    </Dialog>
  );
}
