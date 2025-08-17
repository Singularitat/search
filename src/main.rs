// #![windows_subsystem = "windows"]

use std::{ffi::OsStr, path::Path, sync::mpsc::Receiver, thread, time::Duration};

use eframe::{
    egui::{
        self, Button, ColorImage, FontDefinitions, FontFamily, ImageData, Label, RichText, Sense,
        TextureHandle, TextureOptions,
    },
    epaint::text::{FontInsert, InsertFontFamily},
};
use egui_extras::{Column, TableBuilder};

use filesystem::{FileOrder, FileSystem, SortDirection};

use icon::fetch_and_convert_icon;
use ntfs_reader::{
    api::{ntfs_to_unix_time, NtfsAttributeType},
    journal::{HistorySize, Journal, JournalOptions, NextUsn, UsnRecord},
    mft::Mft,
    volume::Volume,
};
use rustc_hash::FxHashMap;
use windows::{
    core::PCSTR,
    Win32::{
        Storage::FileSystem::{
            GetDriveTypeA, GetLogicalDrives, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        },
        System::Ioctl,
    },
};

mod filesystem;
mod icon;

unsafe fn get_drives() -> Vec<String> {
    let mut drives = Vec::new();
    let mut bitfield = GetLogicalDrives();
    //                              A   :   \  \0
    let mut pathbuf: Vec<u8> = vec![65, 58, 92, 0];

    while bitfield != 0 {
        if bitfield & 1 == 1 {
            let path = std::str::from_utf8_unchecked(&pathbuf);
            let drive_type = GetDriveTypeA(PCSTR(path.as_ptr()));

            if drive_type == 2 || drive_type == 3 {
                // 0..3 to remove the null terminator
                drives.push(path[0..3].to_owned());
            }
        }
        pathbuf[0] += 1;
        bitfield >>= 1;
    }

    drives
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn main() -> Result<(), eframe::Error> {
    let start = std::time::Instant::now();

    let volume = Volume::new(r"\\.\C:").expect("failed to open volume");
    let mft = Mft::new(volume).expect("failed to open mft");

    // possible to miss changes between reading mft and opening journal

    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        let volume = Volume::new(r"\\.\C:").expect("failed to open volume");

        let mut journal = Journal::new(
            volume,
            JournalOptions {
                reason_mask: 0xFFFFFFFF,
                next_usn: NextUsn::Next,
                max_history_size: HistorySize::Limited(4096),
                version_range: (2, 3),
            },
        )
        .expect("failed to open journal");

        loop {
            // let start = std::time::Instant::now();

            if let Ok(records) = journal.read() {
                for record in records {
                    tx.send(record).expect("no receiver");
                }
            }
            // println!("{:?}", start.elapsed());

            thread::sleep(Duration::from_millis(1000));
        }
    });

    let mut filesystem = FileSystem {
        position_mapping: vec![usize::MAX; mft.max_record as usize],
        frn_mapping: Vec::new(),
        parent_mapping: Vec::new(),
        filesizes: Vec::new(),
        modified_dates: Vec::new(),
        filenames: Vec::new(),
        lowercase_filenames: Vec::new(),
        shown: Vec::new(),
        volume_path: r"C:\".into(),
        order: FileOrder::RecordNumber,
        direction: SortDirection::Descending,
    };

    let mut count = 0;

    for number in 0..mft.max_record {
        if let Some(file) = mft.get_record(number) {
            if file.is_used() {
                if let Some(filename) = file.get_best_file_name(&mft) {
                    let parent = filename.parent();
                    let filename = filename.to_string();

                    filesystem.position_mapping[number as usize] = filesystem.filenames.len();

                    filesystem.parent_mapping.push(parent);
                    filesystem.frn_mapping.push(number);

                    let mut accessed = None;
                    let mut created = None;
                    let mut modified = None;
                    let mut size = 0u64;

                    file.attributes(|att| {
                        if att.header.type_id == NtfsAttributeType::StandardInformation as u32 {
                            let stdinfo = att.as_standard_info();

                            accessed = Some(stdinfo.access_time);
                            created = Some(stdinfo.creation_time);
                            modified = Some(stdinfo.modification_time);
                        }

                        if att.header.type_id == NtfsAttributeType::Data as u32 {
                            if att.header.is_non_resident == 0 {
                                size = att.header_res.value_length as u64;
                            } else {
                                size = att.header_nonres.data_size;
                            }
                        }
                    });

                    filesystem.filesizes.push(size);
                    filesystem.modified_dates.push(modified);

                    filesystem
                        .lowercase_filenames
                        .push(filename.to_lowercase().into());
                    filesystem.filenames.push(filename.into());
                }
            } else {
                count += 1;
            }
        }
    }

    println!("{} {}", count, mft.max_record);

    filesystem.shown = (0..filesystem.filenames.len()).collect();

    // manually drop mft as otherwise it will hog memory
    drop(mft);

    println!("Took {:?} to read MFT", start.elapsed());
    println!("{} files", filesystem.filenames.len());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 600.0])
            .with_min_inner_size([100.0, 100.0]),

        ..Default::default()
    };

    eframe::run_native(
        "File Search",
        options,
        Box::new(|cc| {
            cc.egui_ctx.add_font(FontInsert::new(
                "Segoe UI Regular",
                egui::FontData::from_static(include_bytes!(r"C:\Windows\Fonts\segoeui.ttf")),
                vec![
                    InsertFontFamily {
                        family: egui::FontFamily::Proportional,
                        priority: egui::epaint::text::FontPriority::Highest,
                    },
                    InsertFontFamily {
                        family: egui::FontFamily::Monospace,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                ],
            ));

            Ok(Box::new(FileSearch {
                filesystem,
                search: String::new(),
                previous_search: String::new(),
                record_rx: rx,
                icon_cache: FxHashMap::default(),
                default_icon: None,
                folder_icon: None,
            }))
        }),
    )
}

struct FileSearch {
    filesystem: FileSystem,
    search: String,
    previous_search: String,
    record_rx: Receiver<UsnRecord>,
    // --- Icon Cache ---
    icon_cache: FxHashMap<String, Option<TextureHandle>>, // Key: lowercase extension or "<FOLDER>" or "<NO_EXT>"
    default_icon: Option<TextureHandle>,
    folder_icon: Option<TextureHandle>,
}

impl FileSearch {
    fn get_texture_handle(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        // Should maybe store if something is a directory to avoid I/O
        let is_directory = path.is_dir(); // Less efficient, but works for now

        let cache_key: String = if is_directory {
            // Check dedicated folder icon cache first
            if self.folder_icon.is_some() {
                return self.folder_icon.clone();
            }
            "<FOLDER>".to_string()
        } else {
            path.extension()
                .and_then(OsStr::to_str)
                .map_or_else(|| "<NO_EXT>".to_string(), str::to_lowercase)
        };

        // Check general cache
        if let Some(cached_texture_opt) = self.icon_cache.get(&cache_key) {
            return cached_texture_opt.clone();
        }

        let attr_flag = if is_directory {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };

        let texture_opt = unsafe { fetch_and_convert_icon(ctx, path, attr_flag.0) };

        if is_directory {
            self.folder_icon.clone_from(&texture_opt); // cache specific folder icon
        }

        self.icon_cache
            .entry(cache_key) // use the key determined earlier
            .or_insert_with(|| texture_opt.clone()); // use clone here

        texture_opt
    }

    fn get_default_icon(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        if self.default_icon.is_none() {
            // Try to load a truly generic icon using 0 file attributes? Or known file?
            // Let's try getting icon for a non-existent file with .txt extension attributes
            let dummy_path = Path::new("dummy.txt");
            self.default_icon =
                unsafe { fetch_and_convert_icon(ctx, dummy_path, FILE_ATTRIBUTE_NORMAL.0) };

            // Fallback if fetching generic icon fails: create a placeholder egui image
            if self.default_icon.is_none() {
                let fallback_image = ColorImage::new([16, 16], egui::Color32::from_gray(200));
                self.default_icon = Some(ctx.load_texture(
                    "__default_icon__",                      // Use distinct name
                    ImageData::Color(fallback_image.into()), // Use ImageData enum
                    TextureOptions::LINEAR,                  // Use enum variant
                ));
            }
        }
        self.default_icon.clone()
    }
}

impl eframe::App for FileSearch {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.record_rx.try_iter().for_each(|record| {
            // https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v1

            if record.reason & Ioctl::USN_REASON_FILE_DELETE != 0 {
                self.filesystem.delete(record.file_id);
            }

            // The file or directory is renamed, and the file name in the USN_RECORD structure holding this journal record is the new name.
            if record.reason & Ioctl::USN_REASON_RENAME_NEW_NAME != 0 {
                self.filesystem
                    .rename(record.file_id, record.parent_id, &record.path);
            }

            if record.reason & Ioctl::USN_REASON_FILE_CREATE != 0 {
                self.filesystem
                    .create(record.file_id, record.parent_id, &record.path);
            }

            // A user has either changed one or more file or directory attributes
            // (such as the read-only, hidden, system, archive, or sparse attribute), or one or more time stamps.
            if record.reason & Ioctl::USN_REASON_BASIC_INFO_CHANGE != 0 {
                self.filesystem
                    .update(record.file_id, record.parent_id, &record.path);
            }

            // shouldn't need to handle this as we can get all the information we need in the NEW_NAME record
            // The file or directory is renamed, and the file name in the USN_RECORD structure holding this journal record is the previous name
            // if record.reason & Ioctl::USN_REASON_RENAME_OLD_NAME != 0 {}
        });

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            let resp =
                ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(f32::INFINITY));

            if resp.changed() {
                if self.search.is_empty() {
                    self.filesystem.shown = (0..self.filesystem.filenames.len()).collect();
                } else {
                    if !self.previous_search.is_empty()
                        && self.search.contains(&self.previous_search)
                    {
                        // Might have to use starts_with instead of contains
                        // Only search the currently shown files
                        self.filesystem.search_shown(&self.search);
                    } else {
                        self.filesystem.search(&self.search);
                    }
                }
            }

            self.previous_search.clone_from(&self.search);

            ui.separator();
        });

        let total_rows = self.filesystem.shown.len();

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            // ui.separator();

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(format!("{total_rows} files"));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let column_width = ui.available_width() / 2.0;
            let height = ui.available_height();
            let table = TableBuilder::new(ui)
                // .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .max_scroll_height(height) // Without this there is a weird empty space below the table
                .column(Column::exact(column_width.min(400.0)))
                .column(Column::remainder())
                .column(Column::remainder());

            table
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        let is_sorted_by_name = self.filesystem.order == FileOrder::Name;

                        let indicator = if is_sorted_by_name {
                            if self.filesystem.direction == SortDirection::Ascending {
                                " ↑"
                            } else {
                                " ↓"
                            }
                        } else {
                            ""
                        };

                        let name_button =
                            Button::new(RichText::new(format!("Name{}", indicator)).heading())
                                .frame(false);

                        if ui.add(name_button).clicked() {
                            if is_sorted_by_name {
                                self.filesystem.direction =
                                    if self.filesystem.direction == SortDirection::Ascending {
                                        SortDirection::Descending
                                    } else {
                                        SortDirection::Ascending
                                    };

                                self.filesystem.shown.reverse();
                            } else {
                                self.filesystem.order = FileOrder::Name;
                                self.filesystem.direction = SortDirection::Descending;

                                self.filesystem.sort();
                            }
                        }
                    });
                    header.col(|ui| {
                        let is_sorted_by_size = self.filesystem.order == FileOrder::Size;

                        let indicator = if is_sorted_by_size {
                            if self.filesystem.direction == SortDirection::Ascending {
                                " ↑"
                            } else {
                                " ↓"
                            }
                        } else {
                            ""
                        };

                        let size_button =
                            Button::new(RichText::new(format!("File Size{}", indicator)).heading())
                                .frame(false);

                        if ui.add(size_button).clicked() {
                            if is_sorted_by_size {
                                self.filesystem.direction =
                                    if self.filesystem.direction == SortDirection::Ascending {
                                        SortDirection::Descending
                                    } else {
                                        SortDirection::Ascending
                                    };

                                self.filesystem.shown.reverse();
                            } else {
                                self.filesystem.order = FileOrder::Size;
                                self.filesystem.direction = SortDirection::Descending;

                                self.filesystem.sort();
                            }
                        }
                    });
                    header.col(|ui| {
                        ui.heading("Path");
                    });
                })
                .body(|body| {
                    body.rows(18.0, total_rows, |mut row| {
                        let index = self.filesystem.shown[row.index()];

                        let mut full_path = self.filesystem.path(index);

                        let path = full_path.to_string_lossy().to_string();

                        full_path.push(&*self.filesystem.filenames[index]);

                        let icon_texture = self
                            .get_texture_handle(ctx, &full_path)
                            .or_else(|| self.get_default_icon(ctx))
                            .unwrap(); // guaranteed for there to be a default icon

                        row.col(|ui| {
                            let sized_texture =
                                egui::load::SizedTexture::new(icon_texture.id(), (16.0, 16.0));
                            ui.add(egui::Image::from_texture(sized_texture));

                            let resp = ui.add(
                                Label::new(&*self.filesystem.filenames[index])
                                    .sense(Sense::click()),
                            );

                            resp.context_menu(|ui| {
                                if ui.button("Copy path").clicked() {
                                    ui.ctx().copy_text(path.to_string());
                                    ui.close_menu();
                                }
                            });
                        });
                        row.col(|ui| {
                            ui.label(format_size(self.filesystem.filesizes[index]));
                        });
                        row.col(|ui| {
                            // So we can hover to get the full path
                            ui.label(&path).on_hover_text(path);
                        });
                    });
                });
        });
    }
}
