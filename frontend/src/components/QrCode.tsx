import { useMemo } from 'react';
import { encode } from 'uqr';

// Dark modules on white: many scanners reject inverted codes.
export function QrCode({ text }: { text: string }) {
  const { size, path } = useMemo(() => {
    const { data, size } = encode(text, { ecc: 'M', border: 2 });
    let path = '';
    data.forEach((row, y) => row.forEach((dark, x) => dark && (path += `M${x} ${y}h1v1h-1z`)));
    return { size, path };
  }, [text]);
  return (
    <svg viewBox={`0 0 ${size} ${size}`} shapeRendering="crispEdges" className="size-48 rounded-md bg-white">
      <path d={path} fill="black" />
    </svg>
  );
}
