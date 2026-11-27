import { describe, expect, test } from 'vitest'
import { emptyForm, entryToForm, formToEntry, formatServerJson, parseArgs } from './AddMcpDialog'

describe('AddMcpDialog Form <-> JSON conversion', () => {
  test('remote HTTP with OAuth/none builds clean { url } entry and round-trips', () => {
    const f = { ...emptyForm(), name: 'Linear', url: 'https://mcp.linear.app/mcp' }
    const { name, entry } = formToEntry(f)
    expect(name).toBe('linear')
    expect(entry).toEqual({ url: 'https://mcp.linear.app/mcp' })
    const back = entryToForm(name, entry)
    expect(back.name).toBe('linear')
    expect(back.transport).toBe('http')
    expect(back.url).toBe('https://mcp.linear.app/mcp')
    expect(back.authMode).toBe('oauth')
  })

  test('remote HTTP with bearer token emits Authorization: Bearer header and round-trips', () => {
    const f = {
      ...emptyForm(),
      name: 'sentry',
      url: 'https://mcp.sentry.dev/mcp',
      authMode: 'bearer' as const,
      bearerToken: '{' + '{secret:sentry.token}}',
    }
    const { entry } = formToEntry(f)
    expect(entry).toEqual({
      url: 'https://mcp.sentry.dev/mcp',
      headers: { Authorization: 'Bearer {' + '{secret:sentry.token}}' },
    })
    const back = entryToForm('sentry', entry)
    expect(back.authMode).toBe('bearer')
    expect(back.bearerToken).toBe('{' + '{secret:sentry.token}}')
  })

  test('remote HTTP with custom headers emits headers map and round-trips', () => {
    const f = {
      ...emptyForm(),
      name: 'custom-api',
      url: 'https://api.example.com/mcp',
      authMode: 'headers' as const,
      headers: [
        { id: 1, key: 'X-Api-Key', value: '{' + '{secret:api.key}}' },
        { id: 2, key: 'X-Org', value: 'acme' },
      ],
    }
    const { entry } = formToEntry(f)
    expect(entry).toEqual({
      url: 'https://api.example.com/mcp',
      headers: { 'X-Api-Key': '{' + '{secret:api.key}}', 'X-Org': 'acme' },
    })
    const back = entryToForm('custom-api', entry)
    expect(back.authMode).toBe('headers')
    expect(back.headers.map((h) => [h.key, h.value])).toEqual([
      ['X-Api-Key', '{' + '{secret:api.key}}'],
      ['X-Org', 'acme'],
    ])
  })

  test('remote HTTP with advanced OAuth options emits oauth block and round-trips', () => {
    const f = {
      ...emptyForm(),
      name: 'corp',
      url: 'https://mcp.corp.internal/mcp',
      authMode: 'oauth' as const,
      oauthClientId: 'cid-123',
      oauthScopes: 'read write',
      oauthRedirectUri: 'http://127.0.0.1:8765/callback',
    }
    const { entry } = formToEntry(f)
    expect(entry).toEqual({
      url: 'https://mcp.corp.internal/mcp',
      oauth: {
        client_id: 'cid-123',
        scopes: ['read', 'write'],
        redirect_uri: 'http://127.0.0.1:8765/callback',
      },
    })
    const back = entryToForm('corp', entry)
    expect(back.oauthClientId).toBe('cid-123')
    expect(back.oauthScopes).toBe('read write')
    expect(back.oauthRedirectUri).toBe('http://127.0.0.1:8765/callback')
  })

  test('local stdio form builds command/args/cwd/env and round-trips', () => {
    const f = {
      ...emptyForm(),
      name: 'fs',
      transport: 'stdio' as const,
      command: 'npx',
      args: '-y @modelcontextprotocol/server-filesystem "/tmp/my dir"',
      cwd: '/tmp',
      env: [{ id: 1, key: 'DEBUG', value: '1' }],
    }
    expect(parseArgs(f.args)).toEqual(['-y', '@modelcontextprotocol/server-filesystem', '/tmp/my dir'])
    const { entry } = formToEntry(f)
    expect(entry).toEqual({
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-filesystem', '/tmp/my dir'],
      cwd: '/tmp',
      env: { DEBUG: '1' },
    })
    const back = entryToForm('fs', entry)
    expect(back.transport).toBe('stdio')
    expect(back.command).toBe('npx')
    expect(back.cwd).toBe('/tmp')
    expect(back.env[0]?.key).toBe('DEBUG')
  })

  test('formatServerJson produces valid JSON with server key', () => {
    const json = formatServerJson({ ...emptyForm(), name: 'linear', url: 'https://mcp.linear.app/mcp' })
    expect(JSON.parse(json)).toEqual({ linear: { url: 'https://mcp.linear.app/mcp' } })
  })
})
