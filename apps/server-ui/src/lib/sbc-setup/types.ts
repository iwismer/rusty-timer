export interface SbcSetupFormData {
  hostname: string;
  adminUsername: string;
  sshPublicKey: string;
  staticIpv4Cidr: string;
  gateway: string;
  dnsServers: string;
  wifiEnabled: boolean;
  wifiSsid: string;
  wifiPassword: string;
  wifiCountry: string;
  serverBaseUrl: string;
  authToken: string;
  readerTargets: string;
  statusBind: string;
  displayName: string;
}
