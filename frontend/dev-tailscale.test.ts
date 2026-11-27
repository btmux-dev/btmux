import { expect, test } from 'bun:test';
import { parseServeStatus, routeState } from './dev-tailscale';

const host = 'machine.tailnet.ts.net:5173';
const target = 'http://127.0.0.1:5173';
const ours = { TCP: { '5173': { HTTPS: true } }, Web: { [host]: { Handlers: { '/': { Proxy: target } } } } };

test('a node without Serve configuration has none', () => {
  expect(parseServeStatus('')).toEqual({});
  expect(parseServeStatus('\n')).toEqual({});
  expect(parseServeStatus('null\n')).toEqual({});
  expect(() => parseServeStatus('{')).toThrow();
});

test('only a tailnet-only route to Vite is reused', () => {
  expect(routeState({}, host, 5173, target)).toBe('free');
  expect(routeState(ours, host, 5173, target)).toBe('ours');
  expect(routeState({ ...ours, AllowFunnel: { [host]: true } }, host, 5173, target)).toBe('funnel');
  const other = (handlers: object) => ({ TCP: ours.TCP, Web: { [host]: { Handlers: handlers } } });
  expect(routeState(other({ '/': { Proxy: 'http://127.0.0.1:3000' } }), host, 5173, target)).toBe('taken');
  expect(routeState(other({ '/': { Text: 'hi' } }), host, 5173, target)).toBe('taken');
  expect(routeState(other({ '/': { Path: '/srv' } }), host, 5173, target)).toBe('taken');
  expect(routeState(other({ '/': { Proxy: target }, '/x': { Proxy: target } }), host, 5173, target)).toBe('taken');
  // A TCP forward has no Web entry.
  expect(routeState({ TCP: { '5173': { TCPForward: '127.0.0.1:22' } } }, host, 5173, target)).toBe('taken');
});
