# Google Play

The app is **Bothy**, application ID **`com.pocketbothy.app`** (permanent once uploaded). The developer account is a personal one, created after November 2023. That means a **closed test with at least 12 testers, opted in for 14 days in a row**, is required before production access can be requested.

## Signing

- **Play App Signing:** Google holds the key that signs what users install. Ours is the **upload key**, the existing release key: `~/.config/survivalpedia/android-signing/release.jks`, alias `survivalpedia`, RSA 4096, valid until 2126. Its SHA-256 fingerprint:
  `AC:CF:03:9F:74:AC:6A:C1:72:B2:F9:AD:CB:04:03:AF:82:93:D9:91:94:CE:1A:AA:53:26:7E:2C:90:18:F9:C5`
- **CI secrets:** CI signs with it from `ANDROID_KEYSTORE_B64` and `ANDROID_KEYSTORE_PASSWORD`. A Play upload refuses to run without them, because the first upload fixes the upload key.
- **Losing the upload key** isn't fatal: Play support can reset it. Still, keep an offline copy of the `android-signing` folder.
- **Installs from Play and side-loaded CI APKs can't update each other.** They are signed with different keys. Uninstall one to install the other.

## Builds

The `android` workflow builds an APK and an AAB on every push to `master`. Its artifacts are `survivalpedia-apk` and `bothy-aab`. The versionCode is the workflow run number, so it always grows; the versionName comes from `app/pubspec.yaml`.

```
gh run download -R andykcheng/survivalpedia -n bothy-aab                    # newest AAB, for a manual upload
gh workflow run android.yml -R andykcheng/survivalpedia -f play_track=internal   # build + upload (draft)
gh workflow run android.yml -R andykcheng/survivalpedia -f play_track=alpha      # alpha = the closed testing track
```

The upload step needs the `PLAY_SERVICE_ACCOUNT_JSON` secret. Its release status defaults to `draft`, which is required while the app is unpublished; you then roll it out in Console. Release notes come from `app/play/whatsnew/whatsnew-en-GB`.

## One-time setup, in order

1. **Create the app** (Play Console › Create app):
   - name **Bothy**;
   - default language English (United Kingdom) – en-GB;
   - App, Free;
   - accept the declarations.

   Free can never be changed to paid. That's fine, because the app will always be free.
2. **First AAB, by hand:** Testing › Internal testing › Create release. Upload the `bothy-aab` artifact and accept Play App Signing.

   The API can't upload until the app has one build. Until then it answers "Package not found".
3. **Service account, for CI uploads:**
   1. Google Cloud console: create a project (any name), then enable the **Google Play Android Developer API**.
   2. IAM › Service accounts › Create, with no roles. Then Keys › Add key › JSON, which downloads a file.
   3. Play Console › Users and permissions › Invite new users: the service account's email. Under App permissions, add Bothy with **Release apps to testing tracks** (add "Release to production" later) and **View app information**.
   4. Save the key as a secret:

      ```
      gh secret set PLAY_SERVICE_ACCOUNT_JSON -R andykcheng/survivalpedia < key.json
      ```

      Then delete `key.json`.
4. **App content** (Policy › App content). The answers are below.
5. **Store listing.** The text and graphics are below.
6. **Closed test (track `alpha`):**
   - Add testers with a Google Group (easiest) or an email list, and share the opt-in link.
   - At least 12 must stay opted in for 14 days.
   - Then apply for production under Dashboard. Google asks how you tested and what changed.

## Store listing

**Graphics:** `app/play/listing/`, made by `uv run scripts/play_assets.py`.

| File | What it is |
|---|---|
| `icon-512.png` | the icon |
| `feature-1024x500.png` | the feature graphic |
| `phone-1..4.png` | screenshots, 1080×1920 |

Play needs 2 to 8 phone screenshots. The current ones are the app's golden test screens (books, games, notes, share). Real captures from a phone would show the most important screens better: search results, an article, the map, first aid. Each capture's long side must be at most twice the short side, and a phone's full-height screenshot is taller than that, so crop or frame it.

**Category:** Books & Reference. **Tags:** reference, education, travel.

**Contact:** an email address is required and shown publicly. The website is https://pocketbothy.com.

**Privacy policy:** https://pocketbothy.com/privacy

**App name** (30 characters max). DESIGN §14's "Pocket Bothy: Offline Knowledge & First Aid" is 43 characters, too long, so use one of:

> Pocket Bothy: Offline Library (29)
> Pocket Bothy: Offline Guides (28)

**Short description** (80 characters max):

> Offline how-to guides, first aid, maps and books. Works with no internet at all.

**Full description** (4,000 characters max):

> Bothy is a library that lives on your phone. Download it once, and it works anywhere: in a power cut, on a mountain, at sea, abroad without data, or when the internet is down.
>
> WHAT'S INSIDE
> • 131,000 Wikipedia articles chosen for survival and everyday knowledge
> • wikiHow guides: first aid, food, repair, outdoors, home
> • iFixit repair guides, Appropedia and practical Q&A
> • Survival manuals and emergency-care guides
> • First aid cards, a tap away
> • Street maps with place search, country by country
> • Thousands of classic books from Project Gutenberg
> • 22 games for one or more players
> • Notes and checklists, such as a grab-bag list or emergency contacts
>
> SEARCH BY MEANING
> Type what's wrong in your own words, such as "my car won't start in the cold", and Bothy finds the right guide even when the words don't match. Optional AI answers are written from the library, on your phone.
>
> SHARE WITH PEOPLE NEARBY
> Your phone can share the library with people around you. They scan a code and read it in their browser, and there's a chat with photos and voice messages. None of it needs the internet.
>
> BOTHY HUBS
> Set up a Bothy hub on a spare computer or Raspberry Pi, and everyone on your Wi-Fi can use it. Phones can copy content from it. See pocketbothy.com/hub.
>
> FREE, PRIVATE, NO ADS
> Bothy is free, with no ads, accounts or tracking. You choose what to download; everything is stored and searched on your phone.
>
> Content keeps its authors' licences: Wikipedia, Appropedia and Stack Exchange are CC BY-SA, wikiHow and iFixit are CC BY-NC-SA, maps © OpenStreetMap contributors, and the books are public domain from Project Gutenberg.

## App content answers

| Form | Answer |
|---|---|
| Privacy policy | https://pocketbothy.com/privacy |
| Ads | No ads |
| App access | All functionality is available without special access (no login) |
| Content rating (IARC) | Reference/education app. User interaction: **yes**, because nearby chat lets users exchange messages, photos and voice. It's local only and has no moderation. No violence, gambling or purchases. Expect PEGI 12 / Teen-ish because of user interaction. |
| Target audience | 13+ (or 18+). Don't include under-13s: chat with strangers nearby plus an AI would pull in the Families policy. |
| News app | No |
| Health apps | Tick **Medical reference and education** (first aid, medical articles). It's not a medical device. |
| Government app | No |
| Financial features | None |
| Data safety | See below |
| Foreground services | See below |

**Data safety:**
- **Does the app collect or share user data?** No. Nothing is sent to the developer or to third parties. Play's definition of "collected" covers data sent off the device to you or a third party.
- **Nearby sharing:** chat goes phone to phone over local Wi-Fi.
- **Downloads:** standard web requests to data.pocketbothy.com (Cloudflare). Play's guidance doesn't count those as collection.
- **Encryption in transit:** downloads use HTTPS. With no data collected, Play doesn't ask.
- **Account deletion:** not applicable, because the app has no accounts.

**Foreground service declaration** (the manifest has `FOREGROUND_SERVICE_DATA_SYNC` and `FOREGROUND_SERVICE_CONNECTED_DEVICE`). For each type, Play asks what the user sees and wants a short video, which can be an unlisted YouTube link.
- **Data sync:** downloads of collections, maps and books continue with a progress notification while the app is in the background.
- **Connected device:** "Share nearby" keeps the local Wi-Fi hotspot and web server running, so nearby phones stay connected while the screen is off.

**Permissions to explain if asked:**
- Location is used only to start the local Wi-Fi network (an Android requirement before 13) and to share your place in chat when you ask. It's never used in the background, so no background-location declaration is needed.
- The microphone records voice messages in chat.
