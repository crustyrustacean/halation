# Deploying Halation to a DigitalOcean droplet

Target shape: one droplet running the compose stack (app + postgres +
Caddy), DNS on Cloudflare, media in Cloudflare R2, TLS via Let's
Encrypt handled automatically by Caddy.

```
Internet ──> :443 Caddy ──> app:8000 ──> db:5432 (compose network)
                                  └──> Cloudflare R2 (media)
```

## 0. Prerequisites

- A domain (or subdomain) on Cloudflare DNS — any record you control
- An SSH key on the machine you deploy from (`ssh-keygen -t ed25519`
  if you don't have one)
- The four R2 values from Cloudflare (bucket, endpoint, access key,
  secret key)
- Your local machine with Docker (the image is built there and
  transferred — the droplet never compiles)

## 1. Create the droplet

1. Log in to DigitalOcean → green **Create** button (top right) → **Droplets**
2. **Region**: closest to you (e.g. San Francisco)
3. **Image**: OS → **Ubuntu 24.04 (LTS) x64**
4. **Size**: Shared CPU → Basic → **Regular**
   - $6/mo (1 GB) works; **$12/mo (2 GB)** recommended for headroom
5. **Authentication**: SSH Key → **New SSH Key** → paste your public
   key (`cat ~/.ssh/id_ed25519.pub`)
6. Optional but recommended: tick **Backups** (weekly, +20%)
7. Hostname: `halation-prod-1` → **Create Droplet**
8. Copy the **IPv4 address** from the droplet page

## 2. Point DNS at the droplet

Cloudflare dashboard → your domain → **DNS** → **Add record**:

- Type `A`, Name `photos` (or `@` for the root), IPv4 = droplet IP
- Proxy status: **DNS only (grey cloud)** for now — Caddy must face
  Let's Encrypt directly. After HTTPS works you may switch to proxied;
  then set Cloudflare SSL/TLS mode to **Full (strict)**.

## 3. Server setup (over SSH)

```bash
ssh root@<droplet-ip>
curl -fsSL https://get.docker.com | sh
```

## 4. Get the code (deploy key)

```bash
ssh-keygen -t ed25519 -f ~/.ssh/halation_deploy -N ""
cat ~/.ssh/halation_deploy.pub
```

GitHub → the `halation` repo → **Settings → Deploy keys** → **Add
deploy key** → paste the public key (leave read-only). Then:

```bash
cat >> ~/.ssh/config <<'EOF'
Host github.com
  IdentityFile ~/.ssh/halation_deploy
EOF

git clone -b trunk git@github.com:crustyrustacean/halation.git /opt/halation
cd /opt/halation
```

## 5. Configure

Generate secrets locally, then write `.env`:

```bash
openssl rand -hex 24   # POSTGRES_PASSWORD
openssl rand -hex 48   # HALATION_SESSION_SIGNING_KEY
```

```bash
cat > .env <<'EOF'
DOMAIN=photos.example.com                 # ← your subdomain
POSTGRES_PASSWORD=<from openssl above>
HALATION_SESSION_SIGNING_KEY=<from openssl above>
APP_STORAGE__BACKEND=s3
APP_STORAGE__R2__BUCKET=<your bucket>
APP_STORAGE__R2__ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
APP_STORAGE__R2__ACCESS_KEY=<r2 access key id>
APP_STORAGE__R2__SECRET_KEY=<r2 secret access key>
APP_GEOCODE__ENABLED=true
EOF
chmod 600 .env
```

## 6. Transfer the app image (built on your dev machine)

From the dev machine (`~/dev/crustyrustacean/halation`):

```bash
docker build -t halation:latest .
docker save halation:latest | gzip | ssh root@<droplet-ip> 'gunzip | docker load'
```

The droplet never compiles — the image arrives ready to run.

## 7. Launch

```bash
cd /opt/halation
docker compose up -d
docker compose ps                    # db, app, caddy: all running
docker compose exec app curl -sf http://127.0.0.1:8000/health_check
```

The app creates and migrates its database automatically on first boot.

## 8. Verify

- `https://<your-domain>` — the feed page (certificate issued on first visit)
- Register a user, log in, upload a photo
- Check the image objects appear in the Cloudflare R2 bucket
- `GET /api/v1/health` should report `"database": "ok"` and
  `"storage": "ok"`

## 9. Firewall (recommended)

DigitalOcean → **Networking → Firewalls → Create Firewall**:

- Inbound: TCP 22 (SSH — optionally restricted to your IP), TCP 80,
  TCP 443
- Apply to `halation-prod-1`

This sits at the hypervisor, so it can't be bypassed by published
Docker ports.

## 10. Maintenance

- **Deploy an update**: build locally, `docker save | ssh … docker load`
  (step 6), then on the droplet `cd /opt/halation && docker compose up -d`
- **Logs**: `docker compose logs -f app`
- **Database backup** (weekly cron, `crontab -e` as root):
  ```bash
  0 3 * * 0 docker exec halation-db-1 pg_dump -U postgres halation | gzip > /root/backups/halation-$(date +\%F).sql.gz
  ```
  (`mkdir -p /root/backups` first.) Droplet backups via the DO toggle
  cover the rest.
- **Container names** under this compose project: `halation-app-1`,
  `halation-db-1`, `halation-caddy-1`.
