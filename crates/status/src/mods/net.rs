//! ethernet and wireless: GetAdaptersAddresses plus the WLAN API.

use crate::format::{substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use std::ffi::c_void;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IF_TYPE_IEEE80211, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::WiFi::{
    wlan_intf_opcode_current_connection, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WlanQueryInterface, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub up: bool,
    pub wireless: bool,
    pub ipv4: Option<String>,
    pub speed_bps: u64,
}

/// "_first_" = first up adapter of that kind, else FriendlyName (case-insensitive).
pub fn pick_adapter<'a>(list: &'a [Adapter], wanted: &str, wireless: bool) -> Option<&'a Adapter> {
    list.iter().find(|a| {
        a.wireless == wireless
            && if wanted == "_first_" {
                a.up
            } else {
                a.name.eq_ignore_ascii_case(wanted)
            }
    })
}

fn text_of(props: &Props, key: &str, default: &str) -> String {
    get(props, key)
        .map(Value::as_str)
        .unwrap_or_else(|| default.to_string())
}

pub fn ethernet_text(a: Option<&Adapter>, props: &Props) -> (String, Level) {
    match a.filter(|a| a.up) {
        Some(a) => {
            let vars = [
                ("ip", a.ipv4.clone().unwrap_or_else(|| "no IP".to_string())),
                ("speed", format!("{} Mbit/s", a.speed_bps / 1_000_000)),
            ];
            (
                substitute(&text_of(props, "format_up", "E: %ip (%speed)"), &vars),
                Level::Good,
            )
        }
        None => (text_of(props, "format_down", "E: down"), Level::Bad),
    }
}

pub fn wireless_text(
    a: Option<&Adapter>,
    essid: Option<&str>,
    quality: Option<u32>,
    props: &Props,
) -> (String, Level) {
    match a.filter(|a| a.up) {
        Some(a) => {
            let vars = [
                ("ip", a.ipv4.clone().unwrap_or_else(|| "no IP".to_string())),
                ("essid", essid.unwrap_or("").to_string()),
                (
                    "quality",
                    quality.map(|q| format!("{q}%")).unwrap_or_default(),
                ),
            ];
            (
                substitute(
                    &text_of(props, "format_up", "W: (%quality at %essid) %ip"),
                    &vars,
                ),
                Level::Good,
            )
        }
        None => (text_of(props, "format_down", "W: down"), Level::Bad),
    }
}

/// All non-loopback, non-tunnel adapters with their GUID strings (`AdapterName`).
pub fn read_adapters() -> Vec<(Adapter, String)> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..4 {
        buf.resize((size as usize).div_ceil(8), 0);
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                flags,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if rc == 0 {
            return unsafe { walk(buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH) };
        }
        if rc != ERROR_BUFFER_OVERFLOW.0 {
            break;
        }
    }
    Vec::new()
}

unsafe fn walk(first: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<(Adapter, String)> {
    let mut out = Vec::new();
    let mut cur = first;
    while !cur.is_null() {
        let a = &*cur;
        if a.IfType != IF_TYPE_SOFTWARE_LOOPBACK && a.IfType != IF_TYPE_TUNNEL {
            let mut ipv4 = None;
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let sa = (*u).Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == AF_INET {
                    let b = (*(sa as *const SOCKADDR_IN))
                        .sin_addr
                        .S_un
                        .S_addr
                        .to_ne_bytes();
                    ipv4 = Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
                    break;
                }
                u = (*u).Next;
            }
            let guid = if a.AdapterName.is_null() {
                String::new()
            } else {
                String::from_utf8_lossy(a.AdapterName.as_bytes()).into_owned()
            };
            out.push((
                Adapter {
                    name: a.FriendlyName.to_string().unwrap_or_default(),
                    up: a.OperStatus == IfOperStatusUp,
                    wireless: a.IfType == IF_TYPE_IEEE80211,
                    ipv4,
                    speed_bps: a.TransmitLinkSpeed,
                },
                guid,
            ));
        }
        cur = a.Next;
    }
    out
}

fn guid_string(g: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

/// (essid, quality 0-100) of the connected WLAN interface with this adapter GUID.
pub fn read_wifi(adapter_guid: &str) -> Option<(String, u32)> {
    unsafe {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        if WlanOpenHandle(2, None, &mut negotiated, &mut handle) != 0 {
            return None;
        }
        let result = (|| {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(handle, None, &mut list) != 0 || list.is_null() {
                return None;
            }
            let infos = std::slice::from_raw_parts(
                (*list).InterfaceInfo.as_ptr(),
                (*list).dwNumberOfItems as usize,
            );
            let found = infos
                .iter()
                .find(|i| guid_string(&i.InterfaceGuid).eq_ignore_ascii_case(adapter_guid))
                .map(|i| i.InterfaceGuid);
            WlanFreeMemory(list as *const c_void);
            let guid = found?;
            let mut size = 0u32;
            let mut data: *mut c_void = std::ptr::null_mut();
            if WlanQueryInterface(
                handle,
                &guid,
                wlan_intf_opcode_current_connection,
                None,
                &mut size,
                &mut data,
                None,
            ) != 0
                || data.is_null()
            {
                return None;
            }
            let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
            let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
            let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
            let essid = String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned();
            let quality = attrs.wlanAssociationAttributes.wlanSignalQuality;
            WlanFreeMemory(data as *const c_void);
            Some((essid, quality))
        })();
        WlanCloseHandle(handle, None);
        result
    }
}
