import { useId, type SVGProps } from 'react';
import type { IconType } from '@astryxdesign/core/Icon';

// Paths and colors from Lobe Icons; see third-party/lobe-icons/NOTICE.
const CLAUDE =
  'M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z';
const CODEX =
  'M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z';
const GEMINI_ARROW = 'M16.793 10.358v3.867L7.236 18.82v-2.8l7.751-3.728-7.75-3.728V5.763l9.556 4.595z';
const GEMINI_FRAME =
  'M19.608 0A4.392 4.392 0 0124 4.392v15.216A4.392 4.392 0 0119.608 24H4.392A4.392 4.392 0 010 19.608V4.392A4.392 4.392 0 014.392 0h15.216zM4.26 1.444A2.816 2.816 0 001.444 4.26v15.48a2.816 2.816 0 002.816 2.816h15.48a2.816 2.816 0 002.816-2.816V4.26a2.816 2.816 0 00-2.816-2.816H4.26z';
const ANTIGRAVITY =
  'M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z';
const OPENCODE = 'M16 6H8v12h8V6zm4 16H4V2h16v20z';

type Gradient = { x1: number; y1: number; x2: number; y2: number; stops: [number, string][] };

/** A logo of filled paths; `paint` is a color, a gradient, or the text color. */
function glyph(paths: SVGProps<SVGPathElement>[], paint?: string | Gradient): IconType {
  const gradient = typeof paint === 'object' ? paint : undefined;
  const color = typeof paint === 'string' ? paint : 'currentColor';
  return function AgentGlyph(props) {
    const id = useId();
    const fill = gradient ? `url(#${id})` : color;
    return (
      <svg viewBox="0 0 24 24" fill={fill} fillRule="evenodd" {...props}>
        {gradient && (
          <defs>
            <linearGradient
              id={id}
              gradientUnits="userSpaceOnUse"
              x1={gradient.x1}
              y1={gradient.y1}
              x2={gradient.x2}
              y2={gradient.y2}
            >
              {gradient.stops.map(([offset, color]) => (
                <stop key={offset} offset={offset} stopColor={color} />
              ))}
            </linearGradient>
          </defs>
        )}
        {paths.map((path, i) => (
          <path key={i} {...path} />
        ))}
      </svg>
    );
  };
}

// Blurred color blobs masked by the Antigravity shape: [path, color, blur, filter region].
const ANTIGRAVITY_BLOBS: [string, string, number, [number, number, number, number]][] = [
  [
    'M-1.018-3.992c-.408 3.591 2.686 6.89 6.91 7.37 4.225.48 7.98-2.043 8.387-5.633.408-3.59-2.686-6.89-6.91-7.37-4.225-.479-7.98 2.043-8.387 5.633z',
    '#FFE432',
    1.117,
    [-3.288, -11.917, 19.838, 17.587],
  ],
  [
    'M15.269 7.747c1.058 4.557 5.691 7.374 10.348 6.293 4.657-1.082 7.575-5.653 6.516-10.21-1.058-4.556-5.691-7.374-10.348-6.292-4.657 1.082-7.575 5.653-6.516 10.21z',
    '#FC413D',
    5.4,
    [4.251, -13.493, 38.9, 38.565],
  ],
  [
    'M-12.443 10.804c1.338 4.703 7.36 7.11 13.453 5.378 6.092-1.733 9.947-6.95 8.61-11.652C8.282-.173 2.26-2.58-3.833-.848-9.925.884-13.78 6.1-12.443 10.804z',
    '#00B95C',
    4.591,
    [-21.889, -10.592, 40.955, 36.517],
  ],
  [
    'M-12.443 10.804c1.338 4.703 7.36 7.11 13.453 5.378 6.092-1.733 9.947-6.95 8.61-11.652C8.282-.173 2.26-2.58-3.833-.848-9.925.884-13.78 6.1-12.443 10.804z',
    '#00B95C',
    4.591,
    [-21.889, -10.592, 40.955, 36.517],
  ],
  [
    'M-7.608 14.703c3.352 3.424 9.126 3.208 12.896-.483 3.77-3.69 4.108-9.459.756-12.883C2.69-2.087-3.083-1.871-6.853 1.82c-3.77 3.69-4.108 9.458-.755 12.883z',
    '#00B95C',
    4.591,
    [-19.099, -10.278, 36.632, 36.595],
  ],
  [
    'M9.932 27.617c1.04 4.482 5.384 7.303 9.7 6.3 4.316-1.002 6.971-5.448 5.93-9.93-1.04-4.483-5.384-7.304-9.7-6.301-4.316 1.002-6.971 5.448-5.93 9.93z',
    '#3186FF',
    4.363,
    [0.981, 8.758, 33.533, 34.087],
  ],
  [
    'M2.572-8.185C.392-3.329 2.778 2.472 7.9 4.771c5.122 2.3 11.042.227 13.222-4.63 2.18-4.855-.205-10.656-5.327-12.955-5.122-2.3-11.042-.227-13.222 4.63z',
    '#FBBC04',
    3.954,
    [-6.143, -21.659, 35.978, 35.276],
  ],
  [
    'M-3.267 38.686c-5.277-2.072 3.742-19.117 5.984-24.83 2.243-5.712 8.34-8.664 13.616-6.592 5.278 2.071 11.533 13.482 9.29 19.195-2.242 5.713-23.613 14.298-28.89 12.227z',
    '#3186FF',
    3.531,
    [-11.96, -0.46, 45.114, 46.523],
  ],
  [
    'M28.71 17.471c-1.413 1.649-5.1.808-8.236-1.878-3.135-2.687-4.531-6.201-3.118-7.85 1.412-1.649 5.1-.808 8.235 1.878s4.532 6.2 3.119 7.85z',
    '#749BFF',
    3.159,
    [10.485, 0.58, 25.094, 24.054],
  ],
  [
    'M18.163 9.077c5.81 3.93 12.502 4.19 14.946.577 2.443-3.612-.287-9.727-6.098-13.658-5.81-3.931-12.502-4.19-14.946-.577-2.443 3.612.287 9.727 6.098 13.658z',
    '#FC413D',
    2.669,
    [5.833, -12.467, 33.508, 30.007],
  ],
  [
    'M-.915 2.684c-1.44 3.473-.97 6.967 1.05 7.804 2.02.837 4.824-1.3 6.264-4.772 1.44-3.473.97-6.967-1.05-7.804-2.02-.837-4.824 1.3-6.264 4.772z',
    '#FFEE48',
    3.303,
    [-8.355, -8.876, 22.194, 26.151],
  ],
];

function AntigravityLogo(props: SVGProps<SVGSVGElement>) {
  const id = useId();
  return (
    <svg viewBox="0 0 24 24" {...props}>
      <defs>
        <mask id={`${id}m`} maskUnits="userSpaceOnUse" x={0} y={1} width={24} height={23}>
          <path d={ANTIGRAVITY} fill="#fff" />
        </mask>
        {ANTIGRAVITY_BLOBS.map(([, , blur, [x, y, width, height]], i) => (
          <filter
            key={i}
            id={`${id}f${i}`}
            filterUnits="userSpaceOnUse"
            colorInterpolationFilters="sRGB"
            x={x}
            y={y}
            width={width}
            height={height}
          >
            <feGaussianBlur stdDeviation={blur} />
          </filter>
        ))}
      </defs>
      <g mask={`url(#${id}m)`}>
        {ANTIGRAVITY_BLOBS.map(([d, color], i) => (
          <path key={i} d={d} fill={color} filter={`url(#${id}f${i})`} />
        ))}
      </g>
    </svg>
  );
}

/** Display name, monochrome icon, and brand-colored logo per agent id from backend detection. */
export const AGENTS = new Map<string, { name: string; icon: IconType; logo: IconType }>(
  Object.entries({
    claude: {
      name: 'Claude Code',
      icon: glyph([{ clipRule: 'evenodd', d: CLAUDE }]),
      logo: glyph([{ clipRule: 'evenodd', d: CLAUDE }], '#D97757'),
    },
    codex: {
      name: 'Codex',
      icon: glyph([{ clipRule: 'evenodd', d: CODEX }]),
      logo: glyph([{ clipRule: 'evenodd', d: CODEX }], {
        x1: 12,
        y1: 0,
        x2: 12,
        y2: 24,
        stops: [
          [0, '#B1A7FF'],
          [0.5, '#7A9DFF'],
          [1, '#3941FF'],
        ],
      }),
    },
    gemini: {
      name: 'Gemini CLI',
      icon: glyph([{ d: GEMINI_ARROW }, { clipRule: 'evenodd', d: GEMINI_FRAME }]),
      logo: glyph([{ d: GEMINI_ARROW }, { clipRule: 'evenodd', d: GEMINI_FRAME }], {
        x1: 24,
        y1: 6.587,
        x2: 0,
        y2: 16.494,
        stops: [
          [0, '#EE4D5D'],
          [0.328, '#B381DD'],
          [0.476, '#207CFE'],
        ],
      }),
    },
    agy: {
      name: 'Antigravity',
      icon: glyph([{ d: ANTIGRAVITY }]),
      logo: AntigravityLogo,
    },
    opencode: {
      // No brand color: the logo is drawn in the text color.
      name: 'OpenCode',
      icon: glyph([{ d: OPENCODE }]),
      logo: glyph([{ d: OPENCODE }]),
    },
  }),
);
