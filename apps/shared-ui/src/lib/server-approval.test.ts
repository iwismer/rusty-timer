import { describe, expect, it } from 'vitest';
import { approvalBadgeState, serverApprovalTextClass } from './server-approval';

describe('serverApprovalTextClass', () => {
  it('returns warn class while waiting for approval', () => {
    expect(serverApprovalTextClass({ reachable: true, waiting_for_approval: true })).toBe(
      'text-status-warn'
    );
  });

  it('prefers waiting over unreachable', () => {
    expect(serverApprovalTextClass({ reachable: false, waiting_for_approval: true })).toBe(
      'text-status-warn'
    );
  });

  it('returns err class when unreachable', () => {
    expect(serverApprovalTextClass({ reachable: false, waiting_for_approval: false })).toBe(
      'text-status-err'
    );
  });

  it('returns muted class when reachable', () => {
    expect(serverApprovalTextClass({ reachable: true, waiting_for_approval: false })).toBe(
      'text-text-muted'
    );
  });

  it('returns muted class when reachability is unknown', () => {
    expect(serverApprovalTextClass({ reachable: null, waiting_for_approval: false })).toBe(
      'text-text-muted'
    );
  });
});

describe('approvalBadgeState', () => {
  it('returns ok for active', () => {
    expect(approvalBadgeState('active')).toBe('ok');
  });

  it('returns warn for pending', () => {
    expect(approvalBadgeState('pending')).toBe('warn');
  });

  it('returns warn for null', () => {
    expect(approvalBadgeState(null)).toBe('warn');
  });

  it('returns warn for unknown states', () => {
    expect(approvalBadgeState('revoked')).toBe('warn');
  });
});
