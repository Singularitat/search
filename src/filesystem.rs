use std::path::{Path, PathBuf};

use ntfs_reader::journal::FileId;
use rayon::{
    prelude::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator},
    slice::ParallelSliceMut,
};

fn file_id_to_frn(file_id: FileId) -> u64 {
    match file_id {
        FileId::Normal(file_id) => file_id & 0x0000_FFFF_FFFF_FFFF,
        FileId::Extended(file_id_128) => {
            let mut bytes: [u8; 8] = [0; 8];

            bytes[0..6].copy_from_slice(&file_id_128.Identifier[0..6]);

            u64::from_le_bytes(bytes)
        }
    }
}

#[derive(PartialEq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(PartialEq)]
pub enum FileOrder {
    RecordNumber,
    Name,
    ModifedDate,
    Size,
}

pub struct FileSystem {
    // Stores the position of files in the filenames Vec with the index being the FRN
    pub position_mapping: Vec<usize>,
    // Stores the FRN of files with the index being the position in the filesnames Vec
    pub frn_mapping: Vec<u64>,
    // Stores the FRN of the parent with the index being the position in the filenames Vec
    pub parent_mapping: Vec<u64>,
    pub filesizes: Vec<u64>,
    pub modified_dates: Vec<Option<u64>>,
    pub filenames: Vec<Box<str>>,
    // Could use case insensitive regex instead but it is about 2 times slower
    // And takes about 500us to build the regex
    pub lowercase_filenames: Vec<Box<str>>,
    // Maybe use u32 instead of usize since we won't have 2 ** 64 files
    pub shown: Vec<usize>,
    pub volume_path: PathBuf,
    pub order: FileOrder,
    pub direction: SortDirection,
}

impl FileSystem {
    pub fn delete(&mut self, file_id: FileId) {
        let file_record_number = file_id_to_frn(file_id);

        let filename_position = self.position_mapping[file_record_number as usize];

        // idk probably delted it already???
        if filename_position == usize::MAX {
            println!("oop");
            return;
        }

        if filename_position == self.filenames.len() - 1 {
            self.filenames.pop();
            self.lowercase_filenames.pop();

            self.frn_mapping.pop();
            self.parent_mapping.pop();

            self.position_mapping[file_record_number as usize] = usize::MAX;
        } else {
            self.filenames.swap_remove(filename_position);
            self.lowercase_filenames.swap_remove(filename_position);

            // it isn't possible to have 0 files
            let replacement_frn = self.frn_mapping.pop().unwrap();
            self.frn_mapping[filename_position] = replacement_frn;

            let replacement_parent_frn = self.parent_mapping.pop().unwrap();
            self.parent_mapping[filename_position] = replacement_parent_frn;

            self.position_mapping[file_record_number as usize] = usize::MAX;
            self.position_mapping[replacement_frn as usize] = filename_position;
        }

        if let Ok(position) = self.shown.binary_search(&filename_position) {
            // can be very slow but we want it to still be sorted
            self.shown.remove(position);
        }
    }

    pub fn rename(&mut self, file_id: FileId, parent_id: FileId, path: &Path) {
        let file_record_number = file_id_to_frn(file_id);
        let parent_record_number = file_id_to_frn(parent_id);

        let filename_position = self.position_mapping[file_record_number as usize];

        if let Some(filename) = path.file_name() {
            let filename = filename.to_string_lossy();

            // should be able to remove when create/delete are implemented
            if filename_position == usize::MAX {
                return;
            }

            self.lowercase_filenames[filename_position] = filename.to_lowercase().into();
            self.filenames[filename_position] = filename.into();
        }

        self.parent_mapping[filename_position] = parent_record_number;
    }

    pub fn create(&mut self, file_id: FileId, parent_id: FileId, path: &Path) {
        if let Some(filename) = path.file_name() {
            let file_record_number = file_id_to_frn(file_id);
            let parent_record_number = file_id_to_frn(parent_id);

            let filename = filename.to_string_lossy();

            let filename_position = self.filenames.len();

            self.lowercase_filenames
                .push(filename.to_lowercase().into());
            self.filenames.push(filename.to_lowercase().into());

            self.frn_mapping.push(file_record_number);
            self.parent_mapping.push(parent_record_number);

            // expand the position mapping if necessary
            while self.position_mapping.len() as u64 - 1 < file_record_number {
                self.position_mapping.push(usize::MAX);
            }

            self.position_mapping[file_record_number as usize] = filename_position;
        }
    }

    pub fn update(&mut self, file_id: FileId, parent_id: FileId, path: &Path) {}

    pub fn search(&mut self, query: &str) {
        // let start = std::time::Instant::now();

        // self.filenames
        //     .par_iter()
        //     .enumerate()
        //     .for_each(|(i, filename)| {
        //         if filename == &query {
        //             black_box(filename);
        //         }
        //     });

        // println!("Full match {:?}", start.elapsed());

        let start = std::time::Instant::now();

        // Forbidden characters in filenames
        //
        // < (less than)
        // > (greater than)
        // : (colon - sometimes works, but is actually NTFS Alternate Data Streams)
        // " (double quote)
        // / (forward slash)
        // \ (backslash)
        // | (vertical bar or pipe)
        // ? (question mark)
        // * (asterisk)
        //
        // 0-31 (ASCII control characters)
        //
        // Filenames also cannot end in a space or dot.

        let query = query.trim_end().to_ascii_lowercase();

        self.shown = self
            .lowercase_filenames
            .par_iter()
            .enumerate()
            .filter_map(|(i, filename)| filename.contains(&query).then_some(i))
            .collect();

        println!("Searching took {:?}", start.elapsed());

        self.sort();
    }

    pub fn search_shown(&mut self, query: &str) {
        let start = std::time::Instant::now();

        let query = query.trim_end().to_ascii_lowercase();

        self.shown = self
            .shown
            .par_iter()
            .filter_map(|i| {
                unsafe {
                    // This is safe as long as `self.shown` is cleared/updated if a `self.lowercase_filenames` is updated
                    self.lowercase_filenames
                        .get_unchecked(*i)
                        .contains(&query)
                        .then_some(*i)
                }
            })
            .collect();

        println!("Searching shown took {:?}", start.elapsed());

        self.sort();
    }

    pub fn sort(&mut self) {
        let start = std::time::Instant::now();

        match self.order {
            FileOrder::RecordNumber => {
                // since this is just the default with no button to set this there is no direction
                self.shown.sort_unstable();
            }
            FileOrder::Name => {
                self.shown.par_sort_unstable_by(|&a, &b| {
                    let ordering = self.filenames[a].cmp(&self.filenames[b]);

                    match self.direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    }
                });
            }
            FileOrder::ModifedDate => todo!(),
            FileOrder::Size => {
                self.shown.par_sort_unstable_by(|&a, &b| {
                    let ordering = self.filesizes[a].cmp(&self.filesizes[b]);

                    match self.direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    }
                });
            }
        }

        println!("Sorting took: {:?}", start.elapsed());
    }

    pub fn path(&self, position: usize) -> PathBuf {
        let mut filename_position = position;

        let mut components = Vec::new();

        loop {
            let parent = self.parent_mapping[filename_position];

            // Inode #5 is the NTFS root directory
            if parent == 5 {
                break;
            }

            filename_position = self.position_mapping[parent as usize];

            // Not worth using .get_unchecked
            let parent_filename = &self.filenames[filename_position];

            components.push(parent_filename);
        }

        let mut path = self.volume_path.clone();
        for comp in components.iter().rev() {
            path.push(&***comp);
        }

        path
    }
}
