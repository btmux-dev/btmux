import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Trash2 } from 'lucide-react';
import { AlertDialog } from '@astryxdesign/core/AlertDialog';
import { Banner } from '@astryxdesign/core/Banner';
import { Button } from '@astryxdesign/core/Button';
import { CheckboxInput } from '@astryxdesign/core/CheckboxInput';
import { EmptyState } from '@astryxdesign/core/EmptyState';
import { Heading } from '@astryxdesign/core/Heading';
import { useClipboard } from '@astryxdesign/core/hooks';
import { Icon } from '@astryxdesign/core/Icon';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Link } from '@astryxdesign/core/Link';
import { List, ListItem } from '@astryxdesign/core/List';
import { MetadataList, MetadataListItem } from '@astryxdesign/core/MetadataList';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { TextInput } from '@astryxdesign/core/TextInput';
import { Timestamp } from '@astryxdesign/core/Timestamp';
import type { ConnectStatus, PasskeySummary } from '../generated/protocol';
import { useStore } from '../state/store';
import { needsPasskey } from './AuthGate';
import { QrCode } from './QrCode';

const LABEL = { position: 'start', width: 152 } as const;

type Dot = 'success' | 'neutral' | 'warning' | 'error';

function Row({ label, dot, text, detail }: { label: string; dot: Dot; text: string; detail?: ReactNode }) {
  return (
    <MetadataListItem label={label}>
      <VStack gap={1}>
        <HStack gap={2} vAlign="center">
          <StatusDot variant={dot} label={text} />
          <Text>{text}</Text>
        </HStack>
        {detail}
      </VStack>
    </MetadataListItem>
  );
}

/**
 * A relative time that keeps up with the clock. Timestamp compares with the
 * clock it read when mounted, so a new value remounts it.
 */
function LiveTimestamp({ value, format }: { value: number; format: 'relative' | 'auto' }) {
  return <Timestamp key={value} value={value} format={format} isLive type="inherit" color="inherit" />;
}

function Tunnel({ status }: { status: ConnectStatus }) {
  const { tunnel, relay } = status;
  switch (tunnel.state) {
    case 'up':
      return <Row label="Tunnel" dot="success" text={`Connected through ${relay}`} />;
    case 'connecting':
      return <Row label="Tunnel" dot="neutral" text={`Connecting to ${relay}…`} />;
    case 'replaced':
      return (
        <Row
          label="Tunnel"
          dot="warning"
          text="Replaced by another connection"
          detail={<Text color="secondary">Is this registration in use on another machine?</Text>}
        />
      );
    case 'failed':
      return (
        <Row label="Tunnel" dot="error" text="Couldn't start" detail={<Text color="secondary">{tunnel.error}</Text>} />
      );
    case 'down':
      return (
        <Row
          label="Tunnel"
          dot="error"
          text={`Can't reach ${relay}; retrying`}
          detail={<Text color="secondary">{tunnel.error}</Text>}
        />
      );
  }
}

function Certificate({ status }: { status: ConnectStatus }) {
  const { certificate, acme } = status;
  const pending = acme ? 'Self-signed until one is issued.' : 'Browsers warn about it.';
  const certificateRow = !certificate ? (
    <Row
      label="Certificate"
      dot="warning"
      text="No usable certificate"
      detail={<Text color="secondary">{pending}</Text>}
    />
  ) : certificate.self_signed ? (
    <Row label="Certificate" dot="warning" text="Self-signed" detail={<Text color="secondary">{pending}</Text>} />
  ) : (
    <Row
      label="Certificate"
      dot="success"
      text={`Issued by ${certificate.issuer}`}
      detail={
        <Text color="secondary">
          Expires <Timestamp value={certificate.not_after} format="date" type="inherit" color="inherit" />
        </Text>
      }
    />
  );
  return (
    <>
      {certificateRow}
      {acme && (
        <Row
          label="Renewal"
          dot={acme.stopped || acme.error ? 'warning' : 'success'}
          text={acme.directory}
          detail={
            acme.stopped ? (
              <Text color="secondary">The registration changed; restart btmux.</Text>
            ) : acme.error ? (
              <Text color="secondary">{acme.error}</Text>
            ) : acme.renews_at !== null ? (
              <Text color="secondary">
                Renews <LiveTimestamp value={acme.renews_at} format="relative" />
              </Text>
            ) : null
          }
        />
      )}
    </>
  );
}

function Address({ url }: { url: string }) {
  const { copy, isCopied } = useClipboard({ announce: 'Copied' });
  return (
    <MetadataListItem label="Address">
      <HStack gap={1} vAlign="center">
        <Text type="code" className="break-all select-text">
          {url}
        </Text>
        <IconButton
          label={isCopied ? 'Copied' : 'Copy address'}
          tooltip="Copy address"
          variant="ghost"
          size="sm"
          icon={<Icon icon={isCopied ? 'check' : 'copy'} size="sm" color="inherit" />}
          onClick={() => void copy(url)}
        />
      </HStack>
    </MetadataListItem>
  );
}

/** A one-time enrollment link, as `btmux connect-enroll` prints it. */
function EnrollmentLink({ url, expires, onExpired }: { url: string; expires: number; onExpired: () => void }) {
  const { copy, isCopied } = useClipboard({ announce: 'Copied' });
  useEffect(() => {
    const timer = setTimeout(onExpired, Math.max(0, expires * 1000 - Date.now()));
    return () => clearTimeout(timer);
  }, [expires, onExpired]);
  return (
    <VStack gap={2} hAlign="start">
      <QrCode text={url} />
      <Text type="code" className="break-all select-text">
        {url}
      </Text>
      <HStack gap={2}>
        <Button
          label={isCopied ? 'Copied' : 'Copy link'}
          icon={<Icon icon={isCopied ? 'check' : 'copy'} />}
          onClick={() => void copy(url)}
        />
        <Button label="Open" icon={<Icon icon="externalLink" />} href={url} target="_blank" />
      </HStack>
      <Text color="secondary">
        Open it on the device that gets the passkey, or scan it there. It works once and expires{' '}
        <LiveTimestamp value={expires} format="relative" />. Anyone with this link gets a shell on this machine.
      </Text>
    </VStack>
  );
}

/** btmux connect: this machine's address, tunnel, certificate, and passkeys. */
export function ConnectSettings({ status }: { status: ConnectStatus }) {
  const relayed = useStore((s) => s.config?.relayed ?? false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reauth, setReauth] = useState(false);
  const [link, setLink] = useState<{ url: string; expires: number } | null>(null);
  const [removing, setRemoving] = useState<PasskeySummary | null>(null);
  const [signingOut, setSigningOut] = useState(false);
  const enrolled = useRef(status.passkeys.length);

  // A used link has done its job.
  useEffect(() => {
    if (status.passkeys.length > enrolled.current) setLink(null);
    enrolled.current = status.passkeys.length;
  }, [status.passkeys.length]);

  const act = async (name: string, path: string, method: string): Promise<Response | null> => {
    setBusy(name);
    setError(null);
    try {
      const response = await fetch(path, { method, cache: 'no-store' });
      if (needsPasskey(response)) return null;
      if (!response.ok) {
        const message = (await response.text()) || response.statusText;
        // Over the relay, changes take a sign-in under 5 minutes old.
        if (response.status === 403 && message.includes('reauth')) setReauth(true);
        else setError(message);
        return null;
      }
      setReauth(false);
      return response;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'The request failed.');
      return null;
    } finally {
      setBusy(null);
    }
  };

  const enroll = async () => {
    const response = await act('enroll', '/api/connect/enrollments', 'POST');
    if (!response) return;
    const { url, expires_in }: { url: string; expires_in: number } = await response.json();
    setLink({ url, expires: Math.floor(Date.now() / 1000) + expires_in });
  };

  const remove = async (passkey: PasskeySummary) => {
    await act('remove', `/api/connect/passkeys/${encodeURIComponent(passkey.id)}`, 'DELETE');
    setRemoving(null);
  };

  const signOut = async () => {
    await act('sign-out', '/api/connect/sign-out-everywhere', 'POST');
    setSigningOut(false);
  };

  const thisBrowser = relayed ? ' If this browser signed in with it, it is signed out too.' : '';
  return (
    <VStack gap={6}>
      <MetadataList title={<Text type="large">This machine</Text>} label={LABEL}>
        <Address url={status.public_url} />
        <Tunnel status={status} />
        <Certificate status={status} />
      </MetadataList>

      <VStack gap={3}>
        <Heading level={3}>Passkeys</Heading>
        <Text color="secondary">Browsers reaching this machine through the relay sign in with one of these.</Text>
        {reauth && (
          <Banner
            status="warning"
            title="Sign in again to change passkeys"
            description="Over the relay, changes take a sign-in from the last 5 minutes."
            endContent={<Button label="Sign in again" href="/connect/login?reauth" />}
          />
        )}
        {error && <Banner status="error" title={error} />}
        {status.passkeys.length ? (
          <List hasDividers edge="flush">
            {status.passkeys.map((passkey) => (
              <ListItem
                key={passkey.id}
                label={passkey.label}
                description={
                  <Text type="supporting" color="secondary">
                    Added <Timestamp value={passkey.created} format="date" type="inherit" color="inherit" />
                    {passkey.last_used !== null && (
                      <>
                        {' · last used '}
                        <LiveTimestamp value={passkey.last_used} format="auto" />
                      </>
                    )}
                  </Text>
                }
                endContent={
                  <IconButton
                    label={`Remove ${passkey.label}`}
                    tooltip="Remove"
                    variant="ghost"
                    size="sm"
                    icon={<Trash2 />}
                    isDisabled={!!busy}
                    onClick={() => setRemoving(passkey)}
                  />
                }
              />
            ))}
          </List>
        ) : (
          <EmptyState
            isCompact
            title="No passkeys yet"
            description="Add one to sign in to this machine from anywhere."
          />
        )}
        <HStack gap={2}>
          <Button label="Add a passkey" variant="primary" isLoading={busy === 'enroll'} onClick={() => void enroll()} />
          <Button
            label="Sign out everywhere"
            variant="destructive"
            isDisabled={!!busy}
            onClick={() => setSigningOut(true)}
          />
        </HStack>
        {link && <EnrollmentLink url={link.url} expires={link.expires} onExpired={() => setLink(null)} />}
      </VStack>

      <AlertDialog
        isOpen={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title="Remove this passkey?"
        description={`${removing?.label ?? 'It'} can no longer sign in, and browsers signed in with it are signed out.${thisBrowser}`}
        actionLabel="Remove passkey"
        isActionLoading={busy === 'remove'}
        onAction={() => removing && void remove(removing)}
      />
      <AlertDialog
        isOpen={signingOut}
        onOpenChange={setSigningOut}
        title="Sign out everywhere?"
        description={`Every browser signed in through the relay is signed out and its terminals close.${relayed ? ' This browser too.' : ''} Passkeys stay enrolled.`}
        actionLabel="Sign out everywhere"
        isActionLoading={busy === 'sign-out'}
        onAction={() => void signOut()}
      />
    </VStack>
  );
}

const LETS_ENCRYPT_TERMS = 'https://letsencrypt.org/repository/';

/** Sign-up with an invite code, for a server not yet on btmux connect. */
export function ConnectSetup() {
  const [invite, setInvite] = useState('');
  const [agreed, setAgreed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A server without a state file can't keep a registration.
  const [available, setAvailable] = useState(true);
  useEffect(() => {
    let current = true;
    fetch('/api/connect/register', { cache: 'no-store' })
      .then((response) => (response.ok ? response.json() : null))
      .then((body: { available?: boolean } | null) => {
        if (current && body?.available === false) setAvailable(false);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, []);
  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const response = await fetch('/api/connect/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite: invite.trim(), accept_acme_terms: agreed }),
        cache: 'no-store',
      });
      // On success, the pushed status replaces this form.
      if (!response.ok) setError((await response.text()) || response.statusText);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'The request failed.');
    } finally {
      setBusy(false);
    }
  };
  return (
    <VStack gap={4}>
      <VStack gap={1}>
        <Heading level={3}>btmux connect</Heading>
        <Text color="secondary">
          Reach this server from anywhere at its own address, signed in with a passkey. The relay passes connections
          through without decrypting them.
        </Text>
      </VStack>
      {!available && (
        <Banner
          status="info"
          title="Sign-up needs a state directory"
          description="btmux found none (XDG_STATE_HOME or HOME) to keep this server's registration in."
        />
      )}
      {error && <Banner status="error" title={error} />}
      <TextInput
        label="Invite code"
        placeholder="btmi_…"
        value={invite}
        onChange={(value) => setInvite(value)}
        isDisabled={busy || !available}
      />
      <VStack gap={1}>
        <CheckboxInput
          label="I agree to Let's Encrypt's Subscriber Agreement"
          value={agreed}
          onChange={(checked) => setAgreed(checked)}
          isDisabled={busy || !available}
        />
        <Text type="supporting" color="secondary">
          Let&apos;s Encrypt issues this machine&apos;s certificate:{' '}
          <Link href={LETS_ENCRYPT_TERMS} isExternalLink>
            its Subscriber Agreement
          </Link>
          .
        </Text>
      </VStack>
      <HStack>
        <Button
          label="Connect"
          variant="primary"
          isLoading={busy}
          isDisabled={!invite.trim() || !agreed || !available}
          onClick={() => void connect()}
        />
      </HStack>
    </VStack>
  );
}
