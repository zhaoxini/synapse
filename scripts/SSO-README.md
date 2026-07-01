# Synapse SSO (relay)

Email verification, password reset, session management, and Google OAuth for the Synapse relay.

## Features

| Endpoint | Description |
|----------|-------------|
| `POST /api/v1/auth/register` | Create account; sends verification email |
| `POST /api/v1/auth/login` | Email + password sign-in |
| `POST /api/v1/auth/logout` | Invalidate session token |
| `GET /api/v1/auth/me` | Current user profile |
| `POST /api/v1/auth/verify-email` | Confirm 6-digit email code |
| `POST /api/v1/auth/resend-verification` | Resend verification code |
| `POST /api/v1/auth/forgot-password` | Send password reset code |
| `POST /api/v1/auth/reset-password` | Reset password with code |
| `POST /api/v1/auth/change-password` | Change password (authenticated) |
| `GET /api/v1/auth/oauth/google` | Start Google OAuth |
| `GET /api/v1/auth/oauth/google/callback` | OAuth callback (HTML success page) |

Device APIs (`/api/v1/devices`, pairing codes, connect tokens) require a **verified** email.

Existing accounts created before v0.3.0 are grandfathered as verified when the relay DB migrates.

## SMTP (email verification & password reset)

Set these environment variables on the relay service:

```bash
SYNAPSE_SMTP_HOST=smtp.163.com
SYNAPSE_SMTP_PORT=465
SYNAPSE_SMTP_USER=you@163.com
SYNAPSE_SMTP_PASS=your-app-password
SYNAPSE_SMTP_FROM=you@163.com
```

For local testing without SMTP, run the relay with `--dev` — verification codes are logged to stderr instead of emailed.

## Google OAuth

```bash
SYNAPSE_GOOGLE_CLIENT_ID=...
SYNAPSE_GOOGLE_CLIENT_SECRET=...
# optional override (default: https://<public-host>/api/v1/auth/oauth/google/callback)
SYNAPSE_OAUTH_REDIRECT_URI=https://zx0623.duckdns.org/api/v1/auth/oauth/google/callback
```

Register the redirect URI in Google Cloud Console (OAuth 2.0 Web client).

## Example systemd drop-in

```ini
[Service]
Environment=SYNAPSE_SMTP_HOST=smtp.163.com
Environment=SYNAPSE_SMTP_PORT=465
Environment=SYNAPSE_SMTP_USER=you@163.com
Environment=SYNAPSE_SMTP_PASS=secret
Environment=SYNAPSE_SMTP_FROM=you@163.com
Environment=SYNAPSE_GOOGLE_CLIENT_ID=...
Environment=SYNAPSE_GOOGLE_CLIENT_SECRET=...
ExecStart=/usr/local/bin/synapse-relay --host 127.0.0.1 --port 8080 --public-host zx0623.duckdns.org --public-port 443 --public-tls
```

## Client behavior

- **synapse-server** — after register/login, prompts for the email verification code before registering the device.
- **Synapse mobile app** — shows a verify-email screen; supports forgot/reset password and “Continue with Google” (opens browser).
