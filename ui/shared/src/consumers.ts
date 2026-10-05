/** Consumer keys (drainscope-model's `ConsumerKey` storage form), labels and icons. */

export type ConsumerKind =
  | 'app'
  | 'term'
  | 'session'
  | 'shell'
  | 'unit'
  | 'user-unit'
  | 'container'
  | 'other-users'
  | 'root'
  | 'kernel'
  | 'exited'
  | 'idle'
  | 'platform'
  | 'devices'
  | 'drainscope'
  | 'unknown';

export interface Consumer {
  kind: ConsumerKind;
  /** The part after `kind:` (app id, unit name, terminal label…); empty for simple kinds. */
  name: string;
}

const NAMED: readonly ConsumerKind[] = ['app', 'term', 'session', 'unit', 'user-unit', 'container', 'exited'];
const SIMPLE: readonly ConsumerKind[] = [
  'shell',
  'other-users',
  'root',
  'kernel',
  'idle',
  'platform',
  'devices',
  'drainscope',
];

export function parseConsumer(key: string): Consumer {
  const separator = key.indexOf(':');
  if (separator > 0) {
    const kind = NAMED.find((k) => k === key.slice(0, separator));
    const name = key.slice(separator + 1);
    if (kind !== undefined && name !== '') {
      return { kind, name };
    }
  }
  const simple = SIMPLE.find((k) => k === key);
  return simple === undefined ? { kind: 'unknown', name: key } : { kind: simple, name: '' };
}

/** Energy caused by something, as opposed to the idle floor and what RAPL can't see. */
export function isAttributable(consumer: Consumer): boolean {
  return consumer.kind !== 'idle' && consumer.kind !== 'devices' && consumer.kind !== 'platform';
}

const withoutService = (unit: string): string => unit.replace(/\.service$/, '');

/** A readable label; `appName` resolves application ids (e.g. from desktop entries). */
export function describeConsumer(consumer: Consumer, appName: (id: string) => string | undefined): string {
  switch (consumer.kind) {
    case 'app':
      return appName(consumer.name) ?? consumer.name;
    case 'term':
      return `Terminal: ${consumer.name}`;
    case 'session':
      return `Login session ${consumer.name}`;
    case 'shell':
      return 'GNOME Shell';
    case 'unit':
      return `System: ${withoutService(consumer.name)}`;
    case 'user-unit':
      return `Service: ${withoutService(consumer.name)}`;
    case 'container':
      return `Container ${consumer.name}`;
    case 'other-users':
      return 'Other users';
    case 'root':
      return 'Root (sudo, admin sessions)';
    case 'kernel':
      return 'Kernel';
    case 'exited':
      return `Exited processes (${consumer.name})`;
    case 'idle':
      return 'Idle';
    case 'platform':
      return 'Chipset & platform';
    case 'devices':
      return 'Display & devices';
    case 'drainscope':
      return 'drainscope';
    case 'unknown':
      return consumer.name;
  }
}

/** A symbolic icon name for consumers that aren't applications. */
export function iconName(consumer: Consumer): string {
  switch (consumer.kind) {
    case 'term':
      return 'utilities-terminal-symbolic';
    case 'shell':
      return 'video-display-symbolic';
    case 'devices':
      return 'computer-symbolic';
    case 'idle':
      return 'weather-clear-night-symbolic';
    case 'container':
      return 'package-x-generic-symbolic';
    case 'other-users':
    case 'root':
    case 'session':
      return 'system-users-symbolic';
    case 'app':
      return 'application-x-executable-symbolic';
    case 'unit':
    case 'user-unit':
    case 'kernel':
    case 'exited':
    case 'platform':
    case 'drainscope':
    case 'unknown':
      return 'system-run-symbolic';
  }
}
