import { describe, expect, it } from 'vitest';

import type { TableColumns } from '@/ipc/types';
import { toCompletionSchema } from './schemaCache';

const t = (name: string, columns: string[], schema: string | null = null): TableColumns => ({
  schema,
  name,
  columns,
});

describe('toCompletionSchema', () => {
  it('maps table names to their columns', () => {
    const out = toCompletionSchema([t('users', ['id', 'email'])]);
    expect(out).toEqual({ users: ['id', 'email'] });
  });

  it('registers both bare and qualified names so either prefix completes', () => {
    const out = toCompletionSchema([t('users', ['id'], 'public')]);
    expect(out.users).toEqual(['id']);
    expect(out['public.users']).toEqual(['id']);
  });

  it('keeps same-named tables from different schemas distinguishable', () => {
    const out = toCompletionSchema([
      t('users', ['id'], 'public'),
      t('users', ['ts', 'actor'], 'audit'),
    ]);
    // The bare name can only hold one, but both qualified forms survive.
    expect(out['public.users']).toEqual(['id']);
    expect(out['audit.users']).toEqual(['ts', 'actor']);
  });

  it('handles an empty cache', () => {
    expect(toCompletionSchema([])).toEqual({});
  });
});
