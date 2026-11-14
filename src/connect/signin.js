// Passkey enrollment and sign-in for btmux connect (served by connect/passkeys.rs).
'use strict';

const showStatus = (text) => {
  document.getElementById('status').textContent = text;
};

async function post(path, body) {
  const response = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
    cache: 'no-store',
  });
  if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
  return response.status === 204 ? null : response.json();
}

// The server speaks WebAuthn's JSON forms (base64url for binary fields).
const base64url = {
  decode(text) {
    const base64 = text.replace(/-/g, '+').replace(/_/g, '/');
    const binary = atob(base64 + '='.repeat((4 - (base64.length % 4)) % 4));
    return Uint8Array.from(binary, (c) => c.charCodeAt(0)).buffer;
  },
  encode(buffer) {
    let binary = '';
    for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  },
};

function creationOptions(json) {
  if (PublicKeyCredential.parseCreationOptionsFromJSON) return PublicKeyCredential.parseCreationOptionsFromJSON(json);
  return {
    ...json,
    challenge: base64url.decode(json.challenge),
    user: { ...json.user, id: base64url.decode(json.user.id) },
    excludeCredentials: (json.excludeCredentials ?? []).map((c) => ({ ...c, id: base64url.decode(c.id) })),
  };
}

function requestOptions(json) {
  if (PublicKeyCredential.parseRequestOptionsFromJSON) return PublicKeyCredential.parseRequestOptionsFromJSON(json);
  return {
    ...json,
    challenge: base64url.decode(json.challenge),
    allowCredentials: (json.allowCredentials ?? []).map((c) => ({ ...c, id: base64url.decode(c.id) })),
  };
}

function credentialJSON(credential) {
  if (credential.toJSON) return credential.toJSON();
  const response = {};
  for (const name of ['clientDataJSON', 'attestationObject', 'authenticatorData', 'signature', 'userHandle']) {
    if (credential.response[name]) response[name] = base64url.encode(credential.response[name]);
  }
  return {
    id: credential.id,
    rawId: base64url.encode(credential.rawId),
    type: credential.type,
    response,
    clientExtensionResults: credential.getClientExtensionResults(),
  };
}

function failed(error) {
  showStatus(error.name === 'NotAllowedError' ? 'The passkey prompt was dismissed. Try again.' : error.message);
}

async function enroll(button) {
  // The code arrives in the fragment, which never reaches a server or a log.
  const code = new URLSearchParams(location.hash.slice(1)).get('code');
  history.replaceState(null, '', location.pathname);
  if (!code) {
    showStatus('This link has no enrollment code. Ask for a new one.');
    button.disabled = true;
    return;
  }
  let started;
  button.addEventListener('click', async () => {
    button.disabled = true;
    showStatus('');
    try {
      // A dismissed prompt keeps the started enrollment for a retry.
      started ??= await post('/connect/passkeys/register/start', { code });
      const credential = await navigator.credentials.create({ publicKey: creationOptions(started.options.publicKey) });
      await post('/connect/passkeys/register/finish', { id: started.id, credential: credentialJSON(credential) });
      showStatus('Passkey added. Opening btmux…');
      location.replace('/');
    } catch (error) {
      failed(error);
      button.disabled = false;
    }
  });
}

async function login(button) {
  // A Strict cookie stays home on cross-site navigations (a link from chat):
  // if this browser is signed in after all, a same-site reload sends it.
  if ((await fetch('/connect/session', { cache: 'no-store' })).status === 204) {
    location.reload();
    return;
  }
  button.addEventListener('click', async () => {
    button.disabled = true;
    showStatus('');
    try {
      const started = await post('/connect/passkeys/login/start');
      const credential = await navigator.credentials.get({ publicKey: requestOptions(started.options.publicKey) });
      await post('/connect/passkeys/login/finish', { ceremony: started.ceremony, credential: credentialJSON(credential) });
      // This page may stand in for any URL; reloading opens what was asked for.
      location.reload();
    } catch (error) {
      failed(error);
      button.disabled = false;
    }
  });
}

document.addEventListener('DOMContentLoaded', () => {
  if (!window.PublicKeyCredential) {
    showStatus('This browser has no passkey support.');
    return;
  }
  const enrollButton = document.getElementById('enroll');
  const loginButton = document.getElementById('login');
  if (enrollButton) enroll(enrollButton);
  if (loginButton) login(loginButton).catch(failed);
});
