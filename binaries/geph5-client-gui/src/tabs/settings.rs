use std::time::Duration;

use egui::mutex::Mutex;
use geph5_broker_protocol::{BrokerClient, ExitList};
use isocountry::CountryCode;
use itertools::Itertools as _;
use once_cell::sync::Lazy;

use crate::{
    daemon::DAEMON_HANDLE,
    l10n::{l10n, l10n_country},
    refresh_cell::RefreshCell,
    settings::{
        get_config, LANG_CODE, PASSWORD, PROXY_AUTOCONF, SELECTED_CITY, SELECTED_COUNTRY, USERNAME,
        VPN_MODE,
    },
};
use egui_material_icons::icons::*;

pub static LOCATION_LIST: Lazy<Mutex<RefreshCell<ExitList>>> =
    Lazy::new(|| Mutex::new(RefreshCell::new()));

pub fn render_settings(_ctx: &egui::Context, ui: &mut egui::Ui) -> anyhow::Result<()> {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        if ui
            .add(widgets::Button::warning(
                l10n("logout").to_owned(),
                widgets::ButtonSize::Large,
            ))
            .clicked()
        {
            DAEMON_HANDLE.stop()?;
            USERNAME.set("".into());
            PASSWORD.set("".into());
        }
        anyhow::Ok(())
    });

    // Preferences
    ui.separator();

    ui.add(widgets::SettingsLine::new(
        ICON_LANGUAGE.to_string(),
        l10n("language").to_string(),
        Box::new(|ui: &mut egui::Ui| render_language_settings(ui)),
    ));

    // Network settings
    ui.separator();

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    VPN_MODE.modify(|vpn_mode| {
        ui.add(widgets::SettingsLine::new(
            ICON_VPN_KEY.to_string(),
            l10n("vpn_mode").to_string(),
            Box::new(|ui: &mut egui::Ui| ui.add(widgets::Switch::new(vpn_mode))),
        ));
    });

    // #[cfg(not(target_os = "macos"))]
    PROXY_AUTOCONF.modify(|proxy_autoconf| {
        ui.add(widgets::SettingsLine::new(
            ICON_SETTINGS_ETHERNET.to_string(),
            l10n("proxy_autoconf").to_string(),
            Box::new(|ui: &mut egui::Ui| ui.add(widgets::Switch::new(proxy_autoconf))),
        ));
    });

    ui.add(widgets::SettingsLine::new(
        ICON_PLACE.to_string(),
        l10n("exit_location").to_string(),
        Box::new(|ui: &mut egui::Ui| render_location_settings(ui)),
    ));

    Ok(())
}

pub fn render_language_settings(ui: &mut egui::Ui) -> egui::Response {
    let options = vec![
        ("en".to_string(), "English".to_string()),
        ("zh".to_string(), "中文".to_string()),
        ("fa".to_string(), "Fārsī".to_string()),
        ("ru".to_string(), "Русский".to_string()),
    ];
    let lang_code = LANG_CODE.get();

    let mut language = options
        .iter()
        .find(|(code, _)| code == lang_code)
        .map(|(_, name)| name.as_str())
        .unwrap_or(&lang_code)
        .to_string();

    let response = ui.add(widgets::Dropdown::new(
        "language_dropdown",
        options.iter().map(|(_, name)| name.clone()).collect(),
        &mut language,
    ));

    if response.changed() {
        if let Some(lang_code) = options
            .iter()
            .find(|(_, name)| name == &language)
            .map(|(code, _)| code)
        {
            LANG_CODE.set(lang_code.into());
        }
    }

    response
}

pub fn render_location_settings(ui: &mut egui::Ui) -> egui::Response {
    let mut location_list = LOCATION_LIST.lock();
    let locations = location_list.get_or_refresh(Duration::from_secs(10), || {
        smolscale::block_on(async {
            let rpc_transport = get_config().unwrap().broker.unwrap().rpc_transport();
            let client = BrokerClient::from(rpc_transport);
            loop {
                let fallible = async {
                    let exits = client.get_exits().await?.map_err(|e| anyhow::anyhow!(e))?;
                    let mut inner = exits.inner;
                    inner
                        .all_exits
                        .sort_unstable_by_key(|s| (s.1.country, s.1.city.clone()));
                    anyhow::Ok(inner)
                };
                match fallible.await {
                    Ok(v) => return v,
                    Err(err) => tracing::warn!("Failed to get country list: {}", err),
                }
            }
        })
    });

    let country_options = get_country_options(locations);
    let selected_country = SELECTED_COUNTRY.get();
    let city_options = get_city_options(locations, selected_country);

    ui.vertical(|ui| {
        let country_response = ui
            .with_layout(
                egui::Layout::right_to_left(egui::Align::RIGHT),
                |ui: &mut egui::Ui| {
                    let mut current_country = selected_country
                        .map(|c| l10n_country(c).to_string())
                        .unwrap_or_else(|| l10n("auto").to_string());

                    let country_response = ui.add(widgets::Dropdown::new(
                        "country_dropdown",
                        country_options
                            .iter()
                            .map(|(_, name)| name.clone())
                            .collect(),
                        &mut current_country,
                    ));

                    if country_response.changed() {
                        let new_country = if current_country == l10n("auto") {
                            None
                        } else {
                            country_options
                                .iter()
                                .find(|(_, name)| name == &current_country)
                                .and_then(|(code, _)| CountryCode::for_alpha2(code).ok())
                        };
                        SELECTED_COUNTRY.set(new_country);
                        SELECTED_CITY.set(None);
                    }

                    country_response
                },
            )
            .inner;

        let city_response = if selected_country.is_some() {
            Some(
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::RIGHT),
                    |ui: &mut egui::Ui| {
                        let mut current_city = SELECTED_CITY
                            .get()
                            .unwrap_or_else(|| l10n("auto").to_string());

                        let response = ui.add(widgets::Dropdown::new(
                            "city_dropdown",
                            city_options.iter().map(|(_, name)| name.clone()).collect(),
                            &mut current_city,
                        ));

                        if response.changed() {
                            let new_city = if current_city == l10n("auto") {
                                None
                            } else {
                                Some(current_city)
                            };
                            SELECTED_CITY.set(new_city);
                        }

                        response
                    },
                )
                .inner,
            )
        } else {
            None
        };

        city_response.map_or(country_response.clone(), |city_resp| {
            country_response | city_resp
        })
    })
    .inner
}

fn get_country_options(locations: Option<&ExitList>) -> Vec<(String, String)> {
    let mut options = vec![("auto".to_string(), l10n("auto").to_string())];
    if let Some(locations) = locations {
        options.extend(
            locations
                .all_exits
                .iter()
                .map(|s| s.1.country)
                .unique()
                .map(|country| {
                    (
                        country.alpha2().to_string(),
                        l10n_country(country).to_string(),
                    )
                }),
        );
    }
    options
}

fn get_city_options(
    locations: Option<&ExitList>,
    country: Option<CountryCode>,
) -> Vec<(String, String)> {
    let mut options = vec![("auto".to_string(), l10n("auto").to_string())];
    if let Some(locations) = locations {
        if let Some(country) = country {
            options.extend(
                locations
                    .all_exits
                    .iter()
                    .filter(|s| s.1.country == country)
                    .map(|s| &s.1.city)
                    .unique()
                    .map(|city| (city.to_string(), city.to_string())),
            );
        }
    }
    options
}

// pub fn render_broker_settings(ui: &mut egui::Ui) -> anyhow::Result<()> {
//     CUSTOM_BROKER.modify(|custom_broker| {
//         let mut broker_type = match custom_broker {
//             None => 1,
//             Some(BrokerSource::Direct(_)) => 2,
//             Some(BrokerSource::Fronted { front: _, host: _ }) => 3,
//             Some(BrokerSource::DirectTcp(_)) => 4,
//         };
//         ui.vertical(|ui| {
//             egui::ComboBox::from_id_source("custombroker")
//                 .selected_text(match broker_type {
//                     1 => l10n("broker_none"),
//                     2 => l10n("broker_direct"),
//                     3 => l10n("broker_fronted"),
//                     4 => l10n("broker_direct_tcp"),
//                     _ => unreachable!(),
//                 })
//                 .show_ui(ui, |ui| {
//                     ui.selectable_value(&mut broker_type, 1, l10n("broker_none"));
//                     ui.selectable_value(&mut broker_type, 2, l10n("broker_direct"));
//                     ui.selectable_value(&mut broker_type, 3, l10n("broker_fronted"));
//                     ui.selectable_value(&mut broker_type, 4, l10n("broker_direct_tcp"));
//                 });
//             match broker_type {
//                 1 => {
//                     *custom_broker = None;
//                 }
//                 2 => {
//                     let mut addr = if let Some(BrokerSource::Direct(addr)) = custom_broker {
//                         addr.to_owned()
//                     } else {
//                         "".into()
//                     };
//                     ui.text_edit_singleline(&mut addr);
//                     *custom_broker = Some(BrokerSource::Direct(addr));
//                 }
//                 3 => {
//                     let (mut front, mut host) =
//                         if let Some(BrokerSource::Fronted { front, host }) = custom_broker {
//                             (front.to_owned(), host.to_owned())
//                         } else {
//                             ("".into(), "".into())
//                         };
//                     ui.horizontal(|ui| {
//                         ui.label(l10n("broker_fronted_front"));
//                         ui.text_edit_singleline(&mut front);
//                     });
//                     ui.horizontal(|ui| {
//                         ui.label(l10n("broker_fronted_host"));
//                         ui.text_edit_singleline(&mut host);
//                     });
//                     *custom_broker = Some(BrokerSource::Fronted { front, host });
//                 }
//                 4 => {
//                     let mut text = BROKER_DIRECT_TCP_TEXT.lock();
//                     if text.is_none() {
//                         if let Some(BrokerSource::DirectTcp(addr)) = custom_broker {
//                             *text = Some(addr.to_owned().to_string());
//                         } else {
//                             *text = Some("".into());
//                         }
//                     }
//                     ui.text_edit_singleline(text.as_mut().unwrap());
//                     if let Ok(addr) = SocketAddr::from_str(text.clone().unwrap().as_str()) {
//                         *custom_broker = Some(BrokerSource::DirectTcp(addr));
//                     } else {
//                         *custom_broker = Some(BrokerSource::DirectTcp(SocketAddr::V4(
//                             SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0),
//                         )));
//                     }
//                 }
//                 _ => unreachable!(),
//             }
//         });
//     });
//     Ok(())
// }
