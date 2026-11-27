import { useEffect, useState, type SVGProps } from 'react';
import { Copy } from 'lucide-react';
import { Popover } from '@astryxdesign/core/Popover';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Text } from '@astryxdesign/core/Text';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import type { TailscaleStatus } from '../generated/protocol';
import { useStore } from '../state/store';
import { QrCode } from './QrCode';

// Tailscale's nine-dot mark. Sized in em: WebKit collapses an unsized SVG flex item to 0x0.
function TailscaleGlyph(props: SVGProps<SVGSVGElement>) {
  const lit = new Set([3, 4, 5, 7]);
  return (
    <svg viewBox="0 0 24 24" width="1em" height="1em" fill="currentColor" {...props}>
      {Array.from({ length: 9 }, (_, i) => (
        <circle key={i} cx={4 + (i % 3) * 8} cy={4 + Math.floor(i / 3) * 8} r={3} fillOpacity={lit.has(i) ? 1 : 0.3} />
      ))}
    </svg>
  );
}

function copyText(text: string, copied: string, error: string) {
  const { showToast } = useStore.getState();
  const failed = () => showToast(error, 'error');
  // Absent outside secure contexts, e.g. plain HTTP from another machine.
  if (!navigator.clipboard?.writeText) return failed();
  navigator.clipboard.writeText(text).then(() => showToast(copied, 'success'), failed);
}

/** A one-time sign-in link for this tailnet address, renewed before it expires. */
function PairingQr({ url }: { url: string }) {
  const [code, setCode] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const issue = () =>
      fetch('/api/pair', { method: 'POST' })
        .then((response) => (response.ok ? response.json() : Promise.reject(new Error(response.statusText))))
        .then(
          ({ code, expires_in }: { code: string; expires_in: number }) => {
            if (disposed) return;
            setCode(code);
            setFailed(false);
            timer = setTimeout(issue, expires_in * 750);
          },
          () => !disposed && setFailed(true),
        );
    void issue();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, []);
  if (failed) return <Text type="supporting">Could not create a sign-in code.</Text>;
  if (!code) return <div className="size-48" />;
  const link = `${url}/pair?code=${code}`;
  return (
    <VStack gap={1} hAlign="center">
      <QrCode text={link} />
      <HStack gap={1} vAlign="center">
        <Text type="supporting">
          Scan to open btmux on a phone or tablet on your tailnet, or copy the link to another computer. Each code signs
          in once.
        </Text>
        <IconButton
          label="Copy sign-in link"
          tooltip="Copy sign-in link"
          variant="ghost"
          size="sm"
          icon={<Copy />}
          onClick={() => copyText(link, 'Sign-in link copied', 'Could not copy the sign-in link')}
        />
      </HStack>
    </VStack>
  );
}

// Popover content stays mounted while closed; codes are issued only while open.
function Details({ status, open }: { status: TailscaleStatus; open: boolean }) {
  if (status.state === 'starting')
    return (
      <Text color="secondary" className="btm-sans">
        Setting up tailscale serve…
      </Text>
    );
  if (status.state === 'error')
    return (
      <VStack gap={1} className="btm-sans">
        <Text>{status.message}</Text>
        <Text type="supporting">btmux keeps retrying.</Text>
      </VStack>
    );
  return (
    <VStack gap={2} className="btm-sans">
      <HStack gap={1} vAlign="center">
        <Text type="code" className="break-all select-text">
          {status.url}
        </Text>
        <IconButton
          label="Copy address"
          tooltip="Copy address"
          variant="ghost"
          size="sm"
          icon={<Copy />}
          onClick={() =>
            copyText(status.url, 'Tailnet address copied', 'Could not copy the tailnet address; select it to copy it')
          }
        />
      </HStack>
      {/* Relayed tabs can't issue pairing codes, which sign in with the token. */}
      {open && !useStore.getState().config?.relayed && <PairingQr url={status.url} />}
      <Text type="supporting">Or open the address and sign in with this server's access token.</Text>
    </VStack>
  );
}

/** The tailnet address of a server started with `--tailscale`; `lg` is the phone dock's, on the left. */
export function TailscaleButton({ size = 'sm' }: { size?: 'sm' | 'lg' }) {
  const status = useStore((state) => state.tailscale);
  const [open, setOpen] = useState(false);
  if (!status) return null;
  const label =
    status.state === 'serving' ? 'Tailnet' : status.state === 'error' ? 'Tailnet: not serving' : 'Tailnet: starting';
  return (
    <HStack gap={0.5} vAlign="center">
      <Popover
        label="Tailnet"
        placement="above"
        alignment={size === 'lg' ? 'start' : 'end'}
        width={360}
        onOpenChange={setOpen}
        content={<Details status={status} open={open} />}
      >
        <IconButton
          label={label}
          tooltip={label}
          variant="ghost"
          size={size}
          icon={<TailscaleGlyph opacity={status.state === 'serving' ? 1 : 0.5} />}
        />
      </Popover>
      {status.state === 'error' && <StatusDot variant="warning" label="Tailnet not serving" />}
    </HStack>
  );
}
