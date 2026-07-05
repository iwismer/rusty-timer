import type { HelpContext } from './help-types';

export const SERVER_HELP = {
  server_status: {
    title: 'Server Status',
    overview:
      'The status page shows coordination state stored by the server: registered devices, forwarder stream catalogs, and announcer feed health. Timing reads do not normally flow through the server; the server coordinates registration, approval, discovery, and announcer rows.',
    fields: {
      announcer_generation: {
        label: 'Announcer Generation',
        summary: 'Current fenced announcer source generation accepted by the server.',
        detailHtml:
          'A generation number identifies the receiver process currently allowed to publish announcer rows. When a receiver takes over announcer publishing, the generation advances so stale receiver processes cannot overwrite newer rows. You normally do not need to act on this number; if rows stop updating while it keeps changing, confirm that only one receiver has announcer publishing enabled.',
      },
      finisher_count: {
        label: 'Finishers',
        summary: 'Number of unique finishers accepted by the announcer feed.',
        detailHtml:
          "Counts unique chip IDs accepted for the announcer board. Repeat reads of the same chip are ignored, and the visible row list is trimmed to the announcer's configured size, so this count can be higher than the rows shown. These numbers are for announcer assist only; they are not official results.",
      },
      registered_devices_count: {
        label: 'Registered Devices',
        summary: 'Number of forwarders and receivers that have registered with this server.',
        detailHtml:
          'Includes devices in both pending and active approval states. Pending devices have registered but are not yet trusted for normal production traffic.',
      },
    },
    tips: [
      'Use this page as a coordination health check: confirm devices are registered, approved, and reporting stream catalogs before race traffic starts.',
      'If reads are not reaching a receiver, also check the receiver and forwarder UIs because timing data moves over direct P2P sessions.',
    ],
    seeAlso: [
      { sectionKey: 'stream_catalogs', label: 'Stream Catalogs' },
      { sectionKey: 'registered_devices', label: 'Registered Devices' },
    ],
  },
  stream_catalogs: {
    title: 'Forwarder Stream Catalogs',
    overview:
      'Forwarders publish stream catalog metadata to the server so receivers can discover available reader streams and operators can verify what each forwarder is exposing.',
    fields: {
      forwarder: {
        label: 'Forwarder',
        summary: 'The forwarder that reported this stream catalog entry.',
        detailHtml:
          "Shows the forwarder's display name when one is available, with the endpoint ID underneath. Use the endpoint ID for exact verification when two devices have similar names.",
      },
      stream: {
        label: 'Stream',
        summary: 'The stream identifier reported by the forwarder for a reader feed.',
        detailHtml:
          'A stream represents one data feed from a forwarder, typically one IPICO reader. Stream IDs are useful for diagnostics, but operators should usually identify streams by forwarder display name and reader location.',
      },
      epoch: {
        label: 'Epoch',
        summary: 'Current data segment number reported for this stream.',
        detailHtml:
          'The forwarder starts a new epoch when its stream data is reset. If you reset a forwarder mid-event, expect this number to increase; receivers use epochs to avoid mixing old and new reads.',
      },
      next_seq: {
        label: 'Next Seq',
        summary: 'The next sequence number the forwarder expects to assign for this stream.',
        detailHtml:
          'Sequence numbers increase as reads are journaled by the forwarder. An increasing value means the forwarder is journaling new reads for this stream — a quick way to confirm a reader feed is live during testing.',
      },
      approval: {
        label: 'Approval',
        summary: 'Whether the forwarder is pending or active in the server registry.',
        detailHtml:
          'Pending forwarders can publish catalog data for operator review, but receivers can only discover and connect to approved forwarders. Approve known devices from the Admin page.',
      },
    },
    tips: [
      'A forwarder with no streams may be registered but not connected to any reader yet.',
      'Verify endpoint IDs before approving unfamiliar hardware.',
    ],
    seeAlso: [{ sectionKey: 'device_approval', label: 'Device Approval' }],
  },
  registered_devices: {
    title: 'Registered Devices',
    overview:
      'Registered devices are forwarders and receivers known to the server. New devices start pending and must be approved before they participate in normal race-day coordination.',
    fields: {
      endpoint: {
        label: 'Endpoint',
        summary: 'Stable device endpoint identifier used by the registry and P2P coordination.',
        detailHtml:
          'The endpoint ID uniquely identifies a device. Use it when matching a pending device in the Admin page to the physical forwarder or receiver you expect.',
      },
      kind: {
        label: 'Kind',
        summary: 'Whether the device registered as a forwarder or receiver.',
        detailHtml:
          'Forwarders connect to IPICO readers and expose streams. Receivers subscribe to forwarders, proxy reads to local timing software, and can publish announcer rows.',
      },
      approval_state: {
        label: 'Approval',
        summary: 'Pending devices await admin approval; active devices are approved.',
        detailHtml:
          'Pending means the device has registered but is not yet trusted for normal operation. Active means an admin approved it. Only approve devices you recognize.',
      },
    },
    tips: [
      'Keep display names descriptive on devices, but use endpoint IDs for final verification before approval.',
    ],
    seeAlso: [{ sectionKey: 'device_approval', label: 'Device Approval' }],
  },
  receiver_tokens: {
    title: 'Receiver Enrollment Tokens',
    overview:
      'Create one-time enrollment vouchers for receiver apps. A receiver presents the voucher during registration, then the server mints a per-device token for steady-state calls.',
    fields: {
      display_name: {
        label: 'Display Name',
        summary: 'Friendly name attached to the receiver token and pending device.',
        detailHtml:
          'Use a location or role such as <strong>Finish Line</strong> or <strong>Timing Tent</strong>. The name helps operators recognize the receiver when it appears for approval.',
      },
      manual_token: {
        label: 'Manual Token',
        summary: 'Optional pre-shared enrollment secret instead of a generated one.',
        detailHtml:
          'Leave this blank to let the server generate a secure voucher. Enter a manual value only when you need to distribute a pre-agreed secret through another process. Manual tokens must be at least 16 characters long.',
      },
      generate_token: {
        label: 'Generate Token',
        summary: 'Creates a generated one-time receiver enrollment voucher.',
        detailHtml:
          "Generates a receiver enrollment secret and shows it once. Copy it into the receiver app's Config tab along with the server URL and receiver ID.",
      },
      add_manual_token: {
        label: 'Add Manual Token',
        summary: 'Stores the manually entered receiver enrollment voucher.',
        detailHtml:
          'Adds the value from <strong>Manual token</strong> as a receiver voucher. The secret is still treated as one-time enrollment material and is not shown again later.',
      },
      one_time_token: {
        label: 'One-Time Token',
        summary: 'Enrollment secret shown only at creation time.',
        detailHtml:
          'Copy this secret immediately. It is shown only once; existing token rows show metadata such as status and usage but never reveal the secret again.',
      },
      token_status: {
        label: 'Status',
        summary: 'Lifecycle state of the enrollment voucher.',
        detailHtml:
          'Active vouchers can still be used for registration. Used vouchers have already minted a per-device token; the same device can re-present a used voucher for recovery, which resets it to pending for re-approval. Revoked vouchers cannot be used again. Tokens expire 24 hours after creation; expired vouchers are no longer accepted.',
      },
      used_by_endpoint: {
        label: 'Used By Endpoint',
        summary: 'Endpoint ID of the receiver that consumed this voucher.',
        detailHtml:
          'After registration, this identifies the receiver endpoint that used the voucher. Use it to connect token history to a pending or approved device row.',
      },
      revoke_token: {
        label: 'Revoke',
        summary: 'Blocks future registration or recovery using this voucher.',
        detailHtml:
          'Revoking an unused voucher prevents first registration. Revoking a used voucher blocks future recovery or re-registration with that voucher; it does <strong>not</strong> deactivate a receiver that already has a minted per-device token. This page does not currently provide a separate deactivate action for an already-enrolled device.',
      },
    },
    tips: [
      'Prefer generated tokens unless you have a specific reason to pre-share a manual value.',
      'Copy one-time secrets immediately; they cannot be recovered from the token table.',
      'Tokens expire 24 hours after creation. If a receiver was prepared more than a day in advance, create a fresh token before registration.',
    ],
    seeAlso: [{ sectionKey: 'device_approval', label: 'Device Approval' }],
  },
  device_approval: {
    title: 'Device Approval',
    overview:
      'Approve forwarders and receivers after they register. Approval is the operator checkpoint between a device presenting enrollment credentials and being trusted for normal coordination.',
    fields: {
      pending_device: {
        label: 'Pending Device',
        summary: 'A registered device waiting for admin approval.',
        detailHtml:
          'Pending devices have authenticated enough to register, but they are not yet active. Verify the display name, kind, and endpoint ID before approving.',
      },
      approve_device: {
        label: 'Approve',
        summary: 'Marks a known pending device as active.',
        detailHtml:
          'Approving makes the device active in the server registry. Only approve devices you expect to participate in this event.',
      },
      approved_device: {
        label: 'Approved Device',
        summary: 'A device that is active in the server registry.',
        detailHtml:
          'Approved devices can participate in normal server coordination for their device kind, such as receiver discovery, forwarder allow-list fetches, and announcer publishing.',
      },
      endpoint_id: {
        label: 'Endpoint ID',
        summary: 'Exact registry identity for the device.',
        detailHtml:
          'Endpoint IDs are the safest way to distinguish devices with similar display names. Match this ID against the device UI or provisioning notes when approving hardware.',
      },
    },
    tips: [
      'Display names are for humans; endpoint IDs are the final identity check.',
      'If a device is missing from receiver discovery, confirm it is registered and active.',
    ],
    seeAlso: [
      { sectionKey: 'receiver_tokens', label: 'Receiver Enrollment Tokens' },
      { sectionKey: 'sbc_token_management', label: 'SBC Token Management' },
    ],
  },
  sbc_token_management: {
    title: 'Forwarder Token Management',
    overview:
      'Create one-time enrollment vouchers for Raspberry Pi forwarders. Generated tokens can be copied directly into the setup form below.',
    fields: {
      display_name: {
        label: 'Display Name',
        summary: 'Friendly forwarder name attached to the voucher and setup form.',
        detailHtml:
          "Use a physical role such as <strong>Start Line</strong> or <strong>Finish Line A</strong>. When a generated token includes a name, the setup form also uses it as the forwarder's display name.",
      },
      manual_token: {
        label: 'Manual Token',
        summary: 'Optional pre-shared forwarder enrollment secret.',
        detailHtml:
          'Leave blank to generate a secure one-time voucher. Use a manual token only when a separate provisioning process already assigned the secret. Manual tokens must be at least 16 characters long.',
      },
      generate_token: {
        label: 'Generate Token',
        summary: 'Creates a generated one-time forwarder enrollment voucher.',
        detailHtml:
          "Generates a forwarder voucher, shows the secret once, and copies it into the SBC setup form's auth token field so it can be written into <code>user-data</code>.",
      },
      add_manual_token: {
        label: 'Add Manual Token',
        summary: 'Stores the manually entered forwarder voucher.',
        detailHtml:
          "Adds the value from <strong>Manual token</strong> as a forwarder voucher. Like Generate token, the created secret is copied into the setup form's auth token field. After creation, token rows show metadata only and do not reveal the secret.",
      },
      one_time_token: {
        label: 'One-Time Token',
        summary: 'Enrollment secret shown only when it is created.',
        detailHtml:
          'Copy or use this secret immediately. It is shown only once; after you leave this state, the token table keeps only metadata such as status, creation time, and usage.',
      },
      use_in_setup_form: {
        label: 'Use in Setup Form',
        summary: 'Copies the generated voucher into the form below.',
        detailHtml:
          'The generated token is filled in automatically. Use this button only to restore it if the <strong>Auth token</strong> field was cleared, and never re-apply a token that was already used for a different device.',
      },
      revoke_token: {
        label: 'Revoke',
        summary: 'Blocks future registration or recovery using this voucher.',
        detailHtml:
          'Revoking an unused voucher prevents first registration. Revoking a used voucher blocks future per-device token recovery with that voucher; it does not delete existing forwarder status or approval records.',
      },
    },
    tips: [
      'Create a fresh token for each forwarder. Do not reuse one voucher across multiple SBCs.',
      'Tokens expire 24 hours after creation. If you prepared a device more than a day in advance, generate a new token and re-download <code>user-data</code> before flashing.',
      'Download <code>user-data</code> after the correct token is in the setup form.',
    ],
    seeAlso: [{ sectionKey: 'sbc_forwarder_setup', label: 'Forwarder Setup' }],
  },
  sbc_device_identity: {
    title: 'SBC Device Identity',
    overview:
      'Identity and SSH access settings written into the Raspberry Pi cloud-init files for first boot.',
    fields: {
      hostname: {
        label: 'Hostname',
        summary: 'Network hostname for this forwarder device.',
        detailHtml:
          'Use a consistent naming pattern such as <code>rt-fwd-01</code>. <strong>Save &amp; Next Device</strong> increments a trailing number when preparing the next device.',
        default: 'rt-fwd-01',
      },
      admin_username: {
        label: 'SSH Admin Username',
        summary: 'Linux admin account created during first boot.',
        detailHtml:
          'This user is added with sudo access for maintenance and troubleshooting. Usernames must start with a lowercase letter or underscore and use lowercase letters, numbers, underscores, or hyphens.',
        default: 'rt-admin',
      },
      ssh_public_key: {
        label: 'SSH Public Key',
        summary: 'Public key installed for passwordless SSH access.',
        detailHtml:
          'Paste the contents of your public key file, such as <code>~/.ssh/id_ed25519.pub</code>. Password SSH login is disabled by the generated cloud-init file, so a public key is required.',
        recommended: 'Use the same operator key for devices you need to manage together.',
      },
    },
    tips: [
      'Label each physical SBC with its hostname before race day.',
      'Keep the private half of your SSH key secure; only the public key belongs in this form.',
    ],
    seeAlso: [{ sectionKey: 'sbc_network', label: 'Network Configuration' }],
  },
  sbc_network: {
    title: 'SBC Network Configuration',
    overview:
      'Network settings written to network-config. Ethernet is configured with a static address; Wi-Fi can be added as a fallback when needed.',
    fields: {
      static_ipv4_cidr: {
        label: 'Static IPv4/CIDR',
        summary: 'Static Ethernet address and subnet prefix for this SBC.',
        detailHtml:
          'Use CIDR notation such as <code>192.168.1.50/24</code>. Each forwarder needs a unique address on the venue network. <strong>Save &amp; Next Device</strong> can increment the last octet when the hostname ends in a number.',
        default: '192.168.1.50/24',
      },
      gateway: {
        label: 'Default Gateway',
        summary: 'Router address used for non-local network traffic.',
        detailHtml:
          'Usually this is the venue router, such as <code>192.168.1.1</code>. The generated network config installs it as the default route for Ethernet.',
        default: '192.168.1.1',
      },
      dns_servers: {
        label: 'DNS Servers',
        summary: 'Comma-separated IPv4 DNS server list.',
        detailHtml:
          'DNS is used when the server URL contains a hostname. Enter one or more IPv4 addresses separated by commas.',
        default: '8.8.8.8,8.8.4.4',
      },
      wifi_enabled: {
        label: 'Enable Wi-Fi Fallback',
        summary: 'Adds Wi-Fi configuration in addition to static Ethernet.',
        detailHtml:
          'Ethernet remains the primary race-day connection. Enable Wi-Fi only when you need a backup or the venue layout cannot support cable runs.',
        default: 'Disabled',
        recommended: 'Prefer Ethernet for timing reliability.',
      },
      wifi_ssid: {
        label: 'Wi-Fi SSID',
        summary: 'Wi-Fi network name used when fallback is enabled.',
        detailHtml:
          'Required when Wi-Fi fallback is enabled. The generated network config connects <code>wlan0</code> to this network.',
      },
      wifi_country: {
        label: 'Country',
        summary: 'Two-letter Wi-Fi regulatory country code.',
        detailHtml:
          'Use an ISO two-letter country code such as <code>US</code> or <code>CA</code>. Cloud-init writes it as the Wi-Fi regulatory domain.',
        default: 'US',
      },
      wifi_password: {
        label: 'Wi-Fi Password',
        summary: 'Password for the Wi-Fi fallback network.',
        detailHtml:
          'Enter the WPA/WPA2 password. Leave blank only for an open Wi-Fi network. This secret is not saved by <strong>Save &amp; Next Device</strong>.',
      },
    },
    tips: [
      'Reserve a block of static IPs before provisioning multiple forwarders.',
      'Test venue Wi-Fi before relying on it as a fallback.',
    ],
    seeAlso: [{ sectionKey: 'sbc_download_actions', label: 'Download Actions' }],
  },
  sbc_forwarder_setup: {
    title: 'Forwarder Setup',
    overview:
      'Settings written into user-data so the first-boot setup script can install and configure the forwarder service.',
    fields: {
      server_url: {
        label: 'Server URL',
        summary: 'Base URL this forwarder uses for server registration and coordination.',
        detailHtml:
          'The URL must be reachable from the SBC network. The page defaults it to the current server origin when possible. Use HTTPS for deployed systems.',
      },
      auth_token: {
        label: 'Auth Token',
        summary: 'One-time forwarder enrollment voucher written into user-data.',
        detailHtml:
          'Generate or add a token in <strong>Token management</strong>, then make sure the correct secret is in this field before downloading <code>user-data</code>. This secret is not saved by <strong>Save &amp; Next Device</strong>.',
      },
      display_name: {
        label: 'Display Name',
        summary: 'Human-readable forwarder name shown in dashboards.',
        detailHtml:
          "Use the device's race-day location, such as <strong>Start Line</strong> or <strong>Finish Line</strong>. This name helps operators identify streams without reading endpoint IDs.",
        default: 'Start Line',
      },
      reader_targets: {
        label: 'Reader Targets',
        summary: 'IPICO reader addresses this forwarder should connect to.',
        detailHtml:
          'Enter targets as <code>IP:PORT</code> or an end-octet range like <code>192.168.1.10-12:10000</code>. Separate entries with newlines, commas, or semicolons. Most IPICO Lite readers use port 10000; Elite readers may use 10100.',
        default: '192.168.1.10:10000',
      },
    },
    tips: [
      'Double-check reader IPs at the venue before flashing devices.',
      'One token should be used for one forwarder. Generate a new token before preparing the next device.',
    ],
    seeAlso: [
      { sectionKey: 'sbc_token_management', label: 'Forwarder Token Management' },
      { sectionKey: 'sbc_advanced', label: 'Advanced' },
    ],
  },
  sbc_advanced: {
    title: 'SBC Advanced Settings',
    overview:
      'Advanced first-boot settings. Most installs can keep these defaults unless the deployment uses a custom setup path or hardware add-on.',
    fields: {
      status_bind: {
        label: 'Status HTTP Bind',
        summary: "IP and port for the forwarder's local status page.",
        detailHtml:
          "The setup script configures the forwarder's status and control HTTP listener with this value. Use <code>0.0.0.0:80</code> only on a trusted venue network: it makes status, configuration, update, and control endpoints reachable by other devices on that network.",
        default: '0.0.0.0:80',
        range: 'IPv4 address and port',
      },
      setup_script_url: {
        label: 'Setup Script URL',
        summary: 'Shell script downloaded and run during first boot.',
        detailHtml:
          'Cloud-init downloads this script and runs it with the generated environment file. Change it only when testing a custom setup script or fork.',
      },
      ups_enabled: {
        label: 'Enable UPS HAT Support',
        summary: 'Installs support for UPS HAT monitoring on the forwarder.',
        detailHtml:
          'When enabled, first boot installs additional tooling and tells the setup script to configure UPS support. Leave off unless the forwarder has the supported UPS hardware installed.',
        default: 'Disabled',
      },
    },
    tips: [
      'Keep the default setup script URL for normal releases.',
      'Only enable UPS support on hardware that actually has the UPS HAT installed.',
    ],
    seeAlso: [{ sectionKey: 'sbc_forwarder_setup', label: 'Forwarder Setup' }],
  },
  sbc_download_actions: {
    title: 'Download Actions',
    overview:
      'Download the cloud-init files for the SBC boot partition, or prepare the form for provisioning another device.',
    fields: {
      download_user_data: {
        label: 'Download user-data',
        summary:
          'Downloads the cloud-init file that installs and configures the forwarder service.',
        detailHtml:
          'This file contains the setup environment, including the forwarder display name, server URL, enrollment token, reader targets, status bind, and setup script URL. Download it after the form values match the device you are flashing.',
      },
      download_network_config: {
        label: 'Download network-config',
        summary: 'Downloads the cloud-init network configuration file.',
        detailHtml:
          'This file configures Ethernet with the static IPv4/CIDR, gateway, DNS servers, and optional Wi-Fi fallback settings from the form.',
      },
      save_next_device: {
        label: 'Save & Next Device',
        summary: 'Saves non-secret defaults and increments host/IP fields for the next forwarder.',
        detailHtml:
          'Stores non-secret preferences in this browser, clears the auth token, and increments host/IP values like <code>rt-fwd-01</code> and <code>192.168.1.50/24</code> when possible. The Wi-Fi password stays in the form for this session but is never saved to browser storage. Generate a new token before downloading files for the next device.',
      },
    },
    tips: [
      "Download both files for each SBC after entering that device's unique token and IP address.",
      'Secrets are deliberately not saved for the next device.',
    ],
    seeAlso: [
      { sectionKey: 'sbc_device_identity', label: 'SBC Device Identity' },
      { sectionKey: 'sbc_network', label: 'SBC Network Configuration' },
    ],
  },
} as const satisfies HelpContext;
