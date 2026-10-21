// App Store Connect steps for the iOS release channels. Beta uploads label
// each TestFlight build with the apps/ios tree it was built from; promotion
// submits the TestFlight build whose tree matches main, so review receives
// the binary testers ran.
//
//   node appstore.mjs profile <certificate serial>
//   node appstore.mjs next-build
//   node appstore.mjs label <version> <build> <tree> <what to test>
//   node appstore.mjs submit <version> <tree>
//
// Reads HONORUM_ASC_KEY_ID, HONORUM_ASC_ISSUER_ID, and HONORUM_ASC_KEY_PATH.
import { createPrivateKey, sign } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';

const bundleId = 'app.honorum.ios';
const marker = 'Source tree: ';
const [command, ...args] = process.argv.slice(2);
const { HONORUM_ASC_KEY_ID: keyId, HONORUM_ASC_ISSUER_ID: issuer, HONORUM_ASC_KEY_PATH: keyPath } = process.env;
if (!keyId || !issuer || !keyPath) throw new Error('Set HONORUM_ASC_KEY_ID, HONORUM_ASC_ISSUER_ID, and HONORUM_ASC_KEY_PATH');
const key = createPrivateKey(await readFile(keyPath, 'utf8'));

const base64url = value => Buffer.from(value).toString('base64url');
function token() {
  const now = Math.floor(Date.now() / 1000);
  const header = base64url(JSON.stringify({ alg: 'ES256', kid: keyId, typ: 'JWT' }));
  const claims = base64url(JSON.stringify({ iss: issuer, iat: now, exp: now + 600, aud: 'appstoreconnect-v1' }));
  const signature = sign('sha256', Buffer.from(`${header}.${claims}`), { key, dsaEncoding: 'ieee-p1363' });
  return `${header}.${claims}.${signature.toString('base64url')}`;
}

async function api(method, path, body) {
  const response = await fetch(`https://api.appstoreconnect.apple.com${path}`, {
    method,
    headers: { Authorization: `Bearer ${token()}`, 'Content-Type': 'application/json' },
    body: body && JSON.stringify(body),
    signal: AbortSignal.timeout(60_000),
  });
  if (response.status === 204) return null;
  const result = await response.json();
  if (!response.ok) {
    const detail = (result.errors || []).map(error => `${error.title}: ${error.detail}`).join('\n');
    throw new Error(`${method} ${path} returned ${response.status}\n${detail}`);
  }
  return result;
}
const get = path => api('GET', path);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function findApp() {
  const [app] = (await get(`/v1/apps?filter[bundleId]=${bundleId}&fields[apps]=bundleId`)).data;
  if (!app) throw new Error(`App Store Connect has no app record for ${bundleId}`);
  return app;
}
const app = command === 'profile' ? null : await findApp();

async function builds(version) {
  const filter = version ? `&filter[preReleaseVersion.version]=${version}` : '';
  return (await get(`/v1/builds?filter[app]=${app.id}&filter[preReleaseVersion.platform]=IOS${filter}` +
    '&sort=-uploadedDate&limit=200&fields[builds]=version,processingState,expired,uploadedDate')).data;
}

if (command === 'profile') {
  // Use an active App Store profile that contains the imported certificate,
  // creating one when the certificate is new, and install it for xcodebuild.
  const serial = (args[0] || '').toUpperCase().replace(/^0+/, '');
  const certificates = (await get('/v1/certificates?filter[certificateType]=DISTRIBUTION,IOS_DISTRIBUTION' +
    '&fields[certificates]=serialNumber,name&limit=200')).data;
  const certificate = certificates.find(entry => entry.attributes.serialNumber.toUpperCase().replace(/^0+/, '') === serial);
  if (!certificate) throw new Error(`No distribution certificate with serial ${serial} on this team`);
  const identifier = (await get(`/v1/bundleIds?filter[identifier]=${bundleId}&fields[bundleIds]=identifier`)).data
    .find(entry => entry.attributes.identifier === bundleId);
  if (!identifier) throw new Error(`Register ${bundleId} under Certificates, Identifiers & Profiles`);
  const profiles = await get(`/v1/bundleIds/${identifier.id}/profiles?fields[profiles]=name,profileType,profileState,uuid,profileContent&limit=200`);
  let profile;
  for (const candidate of profiles.data) {
    const { profileType, profileState } = candidate.attributes;
    if (profileType !== 'IOS_APP_STORE' || profileState !== 'ACTIVE') continue;
    const members = (await get(`/v1/profiles/${candidate.id}/certificates?fields[certificates]=serialNumber`)).data;
    if (members.some(entry => entry.id === certificate.id)) { profile = candidate; break; }
  }
  profile ??= (await api('POST', '/v1/profiles', { data: {
    type: 'profiles',
    attributes: { name: `Honorum App Store ${serial.slice(-8)}`, profileType: 'IOS_APP_STORE' },
    relationships: {
      bundleId: { data: { type: 'bundleIds', id: identifier.id } },
      certificates: { data: [{ type: 'certificates', id: certificate.id }] },
    },
  } })).data;
  const content = Buffer.from(profile.attributes.profileContent, 'base64');
  for (const directory of ['Library/MobileDevice/Provisioning Profiles', 'Library/Developer/Xcode/UserData/Provisioning Profiles']) {
    await mkdir(`${homedir()}/${directory}`, { recursive: true });
    await writeFile(`${homedir()}/${directory}/${profile.attributes.uuid}.mobileprovision`, content);
  }
  console.log(profile.attributes.name);
} else if (command === 'next-build') {
  // Build numbers read as the UTC upload date and that day's count, such as
  // 2026102103. They also stay above every earlier build, so TestFlight lists
  // them in upload order across versions.
  const numbers = (await builds()).map(build => Number(build.attributes.version)).filter(Number.isSafeInteger);
  const today = Number(new Date().toISOString().slice(0, 10).replaceAll('-', '')) * 100 + 1;
  console.log(Math.max(today, Math.max(0, ...numbers) + 1));
} else if (command === 'label') {
  const [version, number, tree, note] = args;
  const deadline = Date.now() + 45 * 60_000;
  let build;
  while (true) {
    build = (await builds(version)).find(candidate => candidate.attributes.version === number);
    const state = build?.attributes.processingState ?? 'not visible yet';
    if (state === 'VALID') break;
    if (state === 'FAILED' || state === 'INVALID') throw new Error(`Build ${version} (${number}) processing ${state}`);
    if (Date.now() > deadline) throw new Error(`Build ${version} (${number}) still ${state} after 45 minutes`);
    console.log(`Build ${version} (${number}): ${state}`);
    await sleep(30_000);
  }
  const whatToTest = `${note}\n\n${marker}${tree}`;
  const localizations = (await get(`/v1/builds/${build.id}/betaBuildLocalizations`)).data;
  const english = localizations.find(localization => localization.attributes.locale === 'en-US');
  if (english) {
    await api('PATCH', `/v1/betaBuildLocalizations/${english.id}`,
      { data: { type: 'betaBuildLocalizations', id: english.id, attributes: { whatToTest } } });
  } else {
    await api('POST', '/v1/betaBuildLocalizations', { data: {
      type: 'betaBuildLocalizations', attributes: { locale: 'en-US', whatToTest },
      relationships: { build: { data: { type: 'builds', id: build.id } } },
    } });
  }
  console.log(`TestFlight build ${version} (${number}) is ready for internal testers`);
} else if (command === 'submit') {
  const [version, tree] = args;
  let chosen;
  for (const build of await builds(version)) {
    if (build.attributes.processingState !== 'VALID' || build.attributes.expired) continue;
    const localizations = (await get(`/v1/builds/${build.id}/betaBuildLocalizations`)).data;
    if (localizations.some(localization => (localization.attributes.whatToTest || '').includes(marker + tree))) {
      chosen = build;
      break;
    }
  }
  if (!chosen) {
    throw new Error(`No TestFlight build of ${version} was built from apps/ios tree ${tree}. ` +
      'Promote a beta commit after its iOS workflow has finished.');
  }
  const number = chosen.attributes.version;
  console.log(`Submitting TestFlight build ${version} (${number})`);

  const versions = (await get(`/v1/apps/${app.id}/appStoreVersions?filter[platform]=IOS&limit=200` +
    '&fields[appStoreVersions]=versionString,appVersionState&include=build')).data;
  const editable = ['PREPARE_FOR_SUBMISSION', 'DEVELOPER_REJECTED', 'REJECTED', 'METADATA_REJECTED', 'INVALID_BINARY'];
  const shipped = versions.some(entry => ['READY_FOR_DISTRIBUTION', 'REPLACED_WITH_NEW_VERSION'].includes(entry.attributes.appVersionState));
  let target = versions.find(entry => entry.attributes.versionString === version);
  if (target && !editable.includes(target.attributes.appVersionState)) {
    if (target.relationships?.build?.data?.id === chosen.id) {
      console.log(`Version ${version} with build ${number} is already ${target.attributes.appVersionState}`);
      process.exit(0);
    }
    throw new Error(`Version ${version} is ${target.attributes.appVersionState} with another build. ` +
      'Raise MARKETING_VERSION on beta, or cancel that submission in App Store Connect.');
  }
  // App Store Connect allows one editable version at a time, so an unsubmitted
  // version under an older number is renamed rather than duplicated.
  target ??= versions.find(entry => editable.includes(entry.attributes.appVersionState));
  const attributes = { versionString: version, releaseType: 'AFTER_APPROVAL' };
  if (target) {
    await api('PATCH', `/v1/appStoreVersions/${target.id}`, { data: { type: 'appStoreVersions', id: target.id, attributes } });
  } else {
    target = (await api('POST', '/v1/appStoreVersions', { data: {
      type: 'appStoreVersions', attributes: { ...attributes, platform: 'IOS' },
      relationships: { app: { data: { type: 'apps', id: app.id } } },
    } })).data;
  }
  await api('PATCH', `/v1/appStoreVersions/${target.id}/relationships/build`, { data: { type: 'builds', id: chosen.id } });

  // Updates need release notes. The first version cannot have them.
  const notes = await readFile(new URL('../release-notes.txt', import.meta.url), 'utf8').then(text => text.trim(), () => '');
  if (shipped) {
    if (!notes) throw new Error('Write apps/ios/release-notes.txt before promoting an update');
    for (const localization of (await get(`/v1/appStoreVersions/${target.id}/appStoreVersionLocalizations`)).data) {
      await api('PATCH', `/v1/appStoreVersionLocalizations/${localization.id}`,
        { data: { type: 'appStoreVersionLocalizations', id: localization.id, attributes: { whatsNew: notes } } });
    }
  }

  const open = (await get(`/v1/reviewSubmissions?filter[app]=${app.id}&filter[platform]=IOS` +
    '&filter[state]=READY_FOR_REVIEW&fields[reviewSubmissions]=state')).data;
  const submission = open[0] ?? (await api('POST', '/v1/reviewSubmissions', { data: {
    type: 'reviewSubmissions', attributes: { platform: 'IOS' },
    relationships: { app: { data: { type: 'apps', id: app.id } } },
  } })).data;
  const items = (await get(`/v1/reviewSubmissions/${submission.id}/items?include=appStoreVersion`)).data;
  if (!items.some(item => item.relationships?.appStoreVersion?.data?.id === target.id)) {
    await api('POST', '/v1/reviewSubmissionItems', { data: {
      type: 'reviewSubmissionItems',
      relationships: {
        reviewSubmission: { data: { type: 'reviewSubmissions', id: submission.id } },
        appStoreVersion: { data: { type: 'appStoreVersions', id: target.id } },
      },
    } });
  }
  await api('PATCH', `/v1/reviewSubmissions/${submission.id}`,
    { data: { type: 'reviewSubmissions', id: submission.id, attributes: { submitted: true } } });
  console.log(`Submitted ${version} (${number}) for review. It releases automatically once approved.`);
} else {
  throw new Error('Usage: appstore.mjs profile <serial> | next-build | label <version> <build> <tree> <note> | submit <version> <tree>');
}
