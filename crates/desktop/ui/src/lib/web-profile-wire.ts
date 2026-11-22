// The engine's wire types for the shared browser profile (209 §4.4): the one import point for them, so a moved
// generated file is a one-line change here.

export type { Cookie, CookieChange, OriginStorage } from '@/rpc/bindings/method.webprofile.input'
export type {
  ForgottenSite,
  ProfileMode,
  PulledStorage,
  SiteSummary as WebProfileSite,
  WebProfileClearResult,
  WebProfilePullResult,
  WebProfilePushResult,
  WebProfileStatusResult,
} from '@/rpc/bindings/method.webprofile.output'
export type { WebProfileChanged } from '@/rpc/bindings/event.browser_2eweb_2dprofile_2dchanged.output'
