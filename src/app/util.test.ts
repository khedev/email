import { describe, expect, it } from 'vitest';
import { cleanAddressLabel, connectionView, formatBytes, formatDate, formatDateTime, initials, looksLikeGoogleMailbox, oauthClientIdIssue, oauthClientSecretIssue, parseAddresses, stripEmail } from './util';

describe('initials', () => {
  it('builds up-to-two-letter initials', () => {
    expect(initials('John Doe')).toBe('JD');
    expect(initials('Alex')).toBe('A');
    expect(initials('Maria S Kova')).toBe('MS');
    expect(initials('')).toBe('');
  });
});

describe('stripEmail', () => {
  it('extracts the angle-bracket address and trims whitespace', () => {
    expect(stripEmail('John Doe <john@example.com>')).toBe('john@example.com');
    expect(stripEmail('  <a@b.c>  ')).toBe('a@b.c');
    expect(stripEmail('bare@example.com')).toBe('bare@example.com');
  });
});

describe('parseAddresses', () => {
  it('splits a comma-separated recipient string into plain addresses', () => {
    const addresses = parseAddresses('one@x.com, "Two Person <two@x.com>", three@x.com');
    expect(addresses).toEqual(['one@x.com', 'two@x.com', 'three@x.com']);
  });
  it('drops empty entries', () => {
    expect(parseAddresses('a@x.com,,   ,b@y.com')).toEqual(['a@x.com', 'b@y.com']);
  });
});

describe('cleanAddressLabel', () => {
  it('collapses display labels to the address', () => {
    expect(cleanAddressLabel('Update: <c@z.com>')).toBe('c@z.com');
    expect(cleanAddressLabel('Nina Patel <nina@z.com>')).toBe('nina@z.com');
  });
  it('passes bare addresses through', () => {
    expect(cleanAddressLabel(' bare@z.com ')).toBe('bare@z.com');
  });
});

describe('formatBytes', () => {
  it('renders byte sizes with a single unit', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(3 * 1024 * 1024)).toBe('3.0 MB');
    expect(formatBytes(10 * 1024 * 1024)).toBe('10 MB');
  });
});

describe('formatDate', () => {
  it('passes through unparseable values', () => {
    expect(formatDate('not-a-date')).toBe('not-a-date');
  });
  it('returns a short time label for right-now values', () => {
    expect(formatDate(new Date().toISOString()).length).toBeGreaterThan(0);
  });
});

describe('formatDateTime', () => {
  it('passes through unparseable values', () => {
    expect(formatDateTime('not-a-date')).toBe('not-a-date');
  });
  it('always carries a clock time, including for mail received today', () => {
    // `formatDate` alone would collapse this to a bare time and lose the day.
    const stamp = formatDateTime(new Date().toISOString());
    expect(stamp).toMatch(/\d{1,2}:\d{2}/);
    expect(stamp.length).toBeGreaterThan(formatDate(new Date().toISOString()).length);
  });
  it('includes the day and month for older mail', () => {
    const lastYear = new Date();
    lastYear.setFullYear(lastYear.getFullYear() - 2);
    expect(formatDateTime(lastYear.toISOString())).toMatch(/\d{4}/);
  });
});

describe('looksLikeGoogleMailbox', () => {
  it('recognizes Google consumer mailboxes regardless of case or spacing', () => {
    expect(looksLikeGoogleMailbox('someone@gmail.com')).toBe(true);
    expect(looksLikeGoogleMailbox('  Someone@GMail.com ')).toBe(true);
    expect(looksLikeGoogleMailbox('someone@googlemail.com')).toBe(true);
  });
  it('leaves other addresses (including custom Workspace domains) alone', () => {
    expect(looksLikeGoogleMailbox('someone@northstar.test')).toBe(false);
    expect(looksLikeGoogleMailbox('someone@outlook.com')).toBe(false);
    expect(looksLikeGoogleMailbox('gmail.com')).toBe(false);
    expect(looksLikeGoogleMailbox('')).toBe(false);
  });
});

describe('oauthClientIdIssue', () => {
  const valid = '1234567890-abcdefghijklmnopqrstuvwxyz123456.apps.googleusercontent.com';
  it('accepts a well-formed client id', () => {
    expect(oauthClientIdIssue(`  ${valid}  `)).toBeNull();
  });
  it('rejects the client secret, an API key and partial pastes', () => {
    expect(oauthClientIdIssue('GOCSPX-abcdefghijklmnopqrstuvwxyz')).toContain('client secret');
    expect(oauthClientIdIssue('AIzaSyA1234567890abcdefghijklmnopqrstu')).toContain('API key');
    expect(oauthClientIdIssue('1234567890-abcdefghijklmnopqrstuvwxyz12')).toContain('.apps.googleusercontent.com');
    expect(oauthClientIdIssue('1234 5678-abcdefghijklmnopqrstuvwxyz.apps.googleusercontent.com')).toContain('whitespace');
    expect(oauthClientIdIssue('abc.apps.googleusercontent.com')).toContain('wrong length');
    expect(oauthClientIdIssue('')).toContain('Paste the Google OAuth client id');
  });
});

describe('oauthClientSecretIssue', () => {
  it('accepts an empty secret, because desktop / PKCE clients have none', () => {
    expect(oauthClientSecretIssue('')).toBeNull();
    expect(oauthClientSecretIssue('   ')).toBeNull();
  });
  it('accepts a plausible client secret regardless of spacing around it', () => {
    expect(oauthClientSecretIssue('  GOCSPX-abcdefghijklmnopqrstuvwxyz  ')).toBeNull();
  });
  it('rejects the client id, an API key, short and whitespace values', () => {
    expect(oauthClientSecretIssue('1234567890-abcdefghijklmnopqrstuvwxyz123456.apps.googleusercontent.com')).toContain('client id');
    expect(oauthClientSecretIssue('AIzaSyA1234567890abcdefghijklmnopqrstu')).toContain('API key');
    expect(oauthClientSecretIssue('short')).toContain('wrong length');
    expect(oauthClientSecretIssue('GOCSPX-secret with space')).toContain('whitespace');
  });
});

// The connection indicator must tell the truth about two different facts:
// whether the machine has a network, and whether a company transport exists. The
// old one rendered "Offline" forever because it reported the second as the first
// (BUG-015).
describe('connectionView', () => {
  const overview = (state: string, failed = 0) => ({ state, failed, detail: null });

  it('reports a missing network as Offline, whatever the queue says', () => {
    const view = connectionView({ online: false, busy: false, sync: overview('error', 3), health: null });
    expect(view.label).toBe('Offline');
    expect(view.tone).toBe('offline');
    expect(view.hint).toContain('no network');
  });

  it('reports work in progress as Syncing, outranking a stale failure count', () => {
    expect(connectionView({ online: true, busy: true, sync: overview('error', 2), health: null })).toMatchObject({ label: 'Syncing', tone: 'synchronizing' });
    expect(connectionView({ online: true, busy: false, sync: overview('synchronizing'), health: null })).toMatchObject({ label: 'Syncing', tone: 'synchronizing' });
  });

  it('reports failed queue work as a sync error', () => {
    expect(connectionView({ online: true, busy: false, sync: overview('online', 2), health: null })).toMatchObject({ label: 'Sync error', tone: 'error' });
    expect(connectionView({ online: true, busy: false, sync: null, health: { state: 'error' } })).toMatchObject({ label: 'Sync error', tone: 'error' });
  });

  it('reports a quiet store as Connecting until the status has loaded', () => {
    expect(connectionView({ online: true, busy: false, sync: null, health: null })).toMatchObject({ label: 'Connecting', tone: 'connecting' });
  });

  it('states a missing company transport as a configuration fact, not an outage', () => {
    // This is exactly the case that used to render "Offline" with no network
    // problem: the transport is disabled by design.
    const view = connectionView({ online: true, busy: false, sync: overview('offline'), health: { state: 'online' } });
    expect(view.label).toBe('Local only');
    expect(view.tone).toBe('local');
    expect(view.hint).toContain('No company-server transport');
    expect(view.label).not.toContain('Offline');
  });
});