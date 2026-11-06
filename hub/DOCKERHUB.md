# Bothy hub

**An offline library for everyone on your Wi-Fi.** Run it on a spare computer, a mini PC or a Raspberry Pi, and any phone or laptop on the same Wi-Fi can use it in a browser. No internet is needed once it's downloaded. It's useful in a power or internet outage, at a campsite, or anywhere the internet is slow or gone.

What it holds:

- **440,000 articles**: 131,000 Wikipedia articles chosen for survival and everyday knowledge, wikiHow, iFixit repair guides, Appropedia, practical Q&A, and survival guides
- **Search by meaning**, not just keywords ("my car won't start in the cold"), plus optional **AI answers** written from the library
- **First aid cards**
- **Street maps** with place search, country by country (about 500 regions)
- **Books**: up to 54,000 Project Gutenberg books, read in the browser
- **Chat** for everyone on the Wi-Fi, with photos, voice messages and places
- **Games**, and **downloads for phones**: the Bothy Android app copies content from the hub

It's free, has no ads and no accounts, and it doesn't track anyone. The full guide is at **https://pocketbothy.com/hub**.

## Start it

Install Docker ([Docker Desktop](https://www.docker.com/products/docker-desktop/) on Windows or Mac; `curl -fsSL https://get.docker.com | sh` on Linux or a Raspberry Pi), then run:

```
docker run -d --name bothy --restart unless-stopped -p 8080:8080 -p 8443:8443 -v bothy-data:/data -e BOTHY_PRESET=standard andykcheng/bothy
```

On the first start it downloads the library into the `bothy-data` volume. Watch it with `docker logs -f bothy`. The download resumes if it's interrupted, and every file is checked.

| Preset | Size | What's in it |
|---|---|---|
| `small` | 3.2 GB | All the articles, search by meaning, first aid, games, the world map, 1,000 books. No pictures. |
| `standard` | 8.6 GB | Plus pictures for Wikipedia and the guides, AI answers, 10,000 books |
| `everything` | 30 GB | Plus all the wikiHow and iFixit photos, all 54,000 books and illustrated editions |

## Use it

- **On the hub computer:** http://localhost:8080
- **On a phone or laptop on the same Wi-Fi:** `http://<the hub's address>:8080`, e.g. `http://192.168.1.23:8080`
- **Add maps, books and more:** open **Manage** on the hub's page. The first visit asks you to choose an admin password, so do that straight away. Anyone on the Wi-Fi can read, search and chat; only adding or removing content needs the password. It downloads in the background, and the hub switches over without a restart.

Port 8443 is HTTPS (with a self-signed certificate), which chat needs for voice messages and sharing your location.

## Keep it up to date

```
docker exec -it bothy bothy-hub get                       # fetch new or changed content
docker pull andykcheng/bothy && docker rm -f bothy        # new program version, then run the docker run command again
```

Your library stays in the `bothy-data` volume in both cases.

## Options

| Option | What it does |
|---|---|
| `-e BOTHY_PRESET=small` | `small`, `standard` or `everything`, used on the first start |
| `-e BOTHY_ADD=map:uk,map:ireland` | add things on the first start (maps by id, country or name) |
| `-e BOTHY_SKIP=llm` | leave things out (`llm` = AI answers, which need ~3 GB of memory) |
| `-p 8090:8080` | use another port if 8080 is taken |
| `-p 8444:8444 -e BOTHY_HTTPS_PORT=8444` | another HTTPS port; keep the three numbers the same |
| `-v /path/on/disk:/data` | keep the library in a folder you choose |

To change options, run `docker rm -f bothy`, then `docker run` again with the new ones. Your library is kept.

Other commands, all run with `docker exec -it bothy bothy-hub …`:
- `check`: is everything downloaded and ready?
- `maps germany`: find maps; `maps add uk` / `maps remove uk` adds or removes one.
- `reset-password`: forget the Manage password.

## Details

- **Images:** `linux/amd64` and `linux/arm64` (Raspberry Pi 4/5, Apple silicon). The image holds only the program (about 120 MB); content goes into `/data`.
- **Content** downloads from https://data.pocketbothy.com. Each source keeps its licence and attribution: Wikipedia, Appropedia and Stack Exchange are CC BY-SA; wikiHow and iFixit are CC BY-NC-SA (non-commercial); maps are © OpenStreetMap contributors (ODbL); the books are public domain from Project Gutenberg, with its licence inside each book. The full list is in the source repository.
- **Source:** https://github.com/penpapertech/bothy (AGPL-3.0)
