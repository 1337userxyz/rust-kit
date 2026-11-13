//! `kit-csv`: Encoding-sichere CSV-Erkennung, Normalisierung, Suggestions & TSV-Clipboard.

use std::collections::HashMap;
use std::path::Path;

/// Fehler bei CSV-Operationen.
#[derive(Debug, thiserror::Error)]
pub enum KitCsvError {
    #[error("I/O-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV-Fehler: {0}")]
    Csv(#[from] csv::Error),
}

/// Unterstützte CSV-Textencodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvEncoding {
    Utf8,
    Utf8Bom,
    Windows1252,
}

impl CsvEncoding {
    pub fn as_str(&self) -> &'static str {
        match self {
            CsvEncoding::Utf8 => "utf-8",
            CsvEncoding::Utf8Bom => "utf-8-bom",
            CsvEncoding::Windows1252 => "windows-1252",
        }
    }

    pub fn from_label(label: &str) -> Self {
        if label.eq_ignore_ascii_case("utf-8-bom") {
            CsvEncoding::Utf8Bom
        } else if label.eq_ignore_ascii_case("windows-1252") || label.eq_ignore_ascii_case("ansi") {
            CsvEncoding::Windows1252
        } else {
            CsvEncoding::Utf8
        }
    }
}

/// Ergebnis eines gelesenen CSV-Dokuments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvTableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub delimiter: char,
    pub encoding: CsvEncoding,
}

/// Häufigkeits-Eintrag für `KitRichCombo` / `KitEditableCombo` / SmartAdd-Vorschläge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueCountItem {
    pub value: String,
    pub display_label: String,
    pub count: usize,
    pub count_text: String,
}

/// Erkennt das wahrscheinlichste Trennzeichen (`,`, `;`, `\t`, `|`) anhand der ersten 10 nicht-leeren Zeilen.
pub fn sniff_delimiter(bytes: &[u8]) -> char {
    let sample_str = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            cow.into_owned()
        }
    };

    let lines: Vec<&str> = sample_str
        .lines()
        .take(10)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return ',';
    }

    let candidates = [',', ';', '\t', '|'];
    let mut best_delimiter = ',';
    let mut best_score = -1.0;

    for &delim in &candidates {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| l.chars().filter(|&c| c == delim).count())
            .collect();
        if counts.is_empty() || counts[0] == 0 {
            continue;
        }

        let first = counts[0];
        let matching = counts.iter().filter(|&&c| c == first).count();
        let consistency = matching as f64 / counts.len() as f64;
        let score = (first as f64) * 2.0 + consistency * 10.0;

        if score > best_score {
            best_score = score;
            best_delimiter = delim;
        }
    }

    best_delimiter
}

/// Dekodiert Bytes automatisch (UTF-8-BOM → UTF-8 → Windows-1252 Fallback) oder nach Vorgabe.
/// Gibt `(dekodierter_text, erkannter_encoding_name)` zurück.
pub fn decode_bytes(bytes: &[u8], forced_encoding: Option<&str>) -> (String, String) {
    if let Some(enc) = forced_encoding {
        if enc.eq_ignore_ascii_case("windows-1252") || enc.eq_ignore_ascii_case("ansi") {
            let (res, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            return (res.into_owned(), "windows-1252".to_string());
        }
        if enc.eq_ignore_ascii_case("utf-8") || enc.eq_ignore_ascii_case("utf8") {
            let stripped = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            let (res, _, _) = encoding_rs::UTF_8.decode(stripped);
            return (res.into_owned(), "utf-8".to_string());
        }
        if enc.eq_ignore_ascii_case("utf-8-bom") {
            let stripped = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            let (res, _, _) = encoding_rs::UTF_8.decode(stripped);
            return (res.into_owned(), "utf-8-bom".to_string());
        }
    }

    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let (res, _, _) = encoding_rs::UTF_8.decode(&bytes[3..]);
        return (res.into_owned(), "utf-8-bom".to_string());
    }

    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), "utf-8".to_string()),
        Err(_) => {
            let (res, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            (res.into_owned(), "windows-1252".to_string())
        }
    }
}

/// Liest eine Textdatei mit automatischer Encoding-Erkennung (UTF-8, UTF-8-BOM, Windows-1252).
pub fn read_text_auto(path: &Path, forced_encoding: Option<&str>) -> std::io::Result<(String, String)> {
    let bytes = std::fs::read(path)?;
    Ok(decode_bytes(&bytes, forced_encoding))
}

/// Erstellt eine case-insensitive & getrimmte Mapping-Tabelle von Header-Name auf Spaltenindex.
pub fn build_header_map(headers: &[String]) -> HashMap<String, usize> {
    let mut map = HashMap::with_capacity(headers.len());
    for (idx, h) in headers.iter().enumerate() {
        let key = h.trim().to_lowercase();
        map.entry(key).or_insert(idx);
    }
    map
}

/// Liest Header und Zeilen einer CSV-Datei mit automatischem Delimiter/Encoding und überspringt leere Zeilen.
pub fn read_csv_records(
    path: &Path,
    delimiter: Option<char>,
    encoding: Option<&str>,
) -> Result<CsvTableData, KitCsvError> {
    let bytes = std::fs::read(path)?;
    let delim = delimiter.unwrap_or_else(|| sniff_delimiter(&bytes));
    let (text, detected_enc) = decode_bytes(&bytes, encoding);

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim as u8)
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());

    let headers: Vec<String> = rdr.headers()?.iter().map(|s| s.to_string()).collect();
    let mut max_cols = headers.len();
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result?;
        let row_vec: Vec<String> = record.iter().map(|s| s.to_string()).collect();
        if row_vec.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if row_vec.len() > max_cols {
            max_cols = row_vec.len();
        }
        rows.push(row_vec);
    }

    for row in &mut rows {
        if row.len() < max_cols {
            row.resize(max_cols, String::new());
        }
    }

    Ok(CsvTableData {
        headers,
        rows,
        delimiter: delim,
        encoding: CsvEncoding::from_label(&detected_enc),
    })
}

/// Speichert CSV-Daten normalisiert und atomar (inkl. optionalem `.bak`-Backup via `kit-fs`).
pub fn save_csv_normalized(
    path: &Path,
    headers: &[String],
    rows: &[Vec<String>],
    delimiter: char,
    encoding: CsvEncoding,
    skip_empty_rows: bool,
    create_bak: bool,
) -> Result<(), KitCsvError> {
    let mut buf = Vec::new();
    {
        let mut wtr = csv::WriterBuilder::new()
            .delimiter(delimiter as u8)
            .from_writer(&mut buf);

        wtr.write_record(headers)?;

        for row in rows {
            if skip_empty_rows && row.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let record: Vec<&str> = (0..headers.len())
                .map(|i| row.get(i).map(|s| s.as_str()).unwrap_or(""))
                .collect();
            wtr.write_record(&record)?;
        }
        wtr.flush()?;
    }

    let output_bytes = match encoding {
        CsvEncoding::Windows1252 => {
            let text = String::from_utf8_lossy(&buf);
            let (cow, _, _) = encoding_rs::WINDOWS_1252.encode(&text);
            cow.into_owned()
        }
        CsvEncoding::Utf8Bom => {
            let mut out = Vec::with_capacity(3 + buf.len());
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            out.extend_from_slice(&buf);
            out
        }
        CsvEncoding::Utf8 => buf,
    };

    kit_fs::write_atomic_with_backup(path, &output_bytes, create_bak)?;
    Ok(())
}

/// Analysiert die Werte einer Spalte, filtert optional nach `filter_query`,
/// sortiert nach Häufigkeit absteigend (danach alphabetisch) und begrenzt auf `max_items` (0 = unbegrenzt).
pub fn analyze_column_suggestions(
    rows: &[Vec<String>],
    col_idx: usize,
    filter_query: &str,
    max_items: usize,
) -> Vec<ValueCountItem> {
    let query = filter_query.trim().to_lowercase();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for row in rows {
        if let Some(cell) = row.get(col_idx) {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !query.is_empty() && !trimmed.to_lowercase().contains(&query) {
                continue;
            }
            *counts.entry(trimmed.to_string()).or_insert(0) += 1;
        }
    }

    let mut pairs: Vec<(String, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
            .then_with(|| a.0.cmp(&b.0))
    });

    if max_items > 0 && pairs.len() > max_items {
        pairs.truncate(max_items);
    }

    pairs
        .into_iter()
        .map(|(value, count)| ValueCountItem {
            display_label: format!("{value} ({count})"),
            count_text: format!("{count}×"),
            value,
            count,
        })
        .collect()
}

/// Exportiert Tabellenzeilen als Excel-kompatiblen TSV-String (Tab-getrennt, `\n` Zeilenumbruch).
pub fn to_tsv_clipboard(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|r| r.join("\t"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parst einen Clipboard-String (Tab- oder Komma-getrennt) in Zeilen und Zellen.
pub fn from_tsv_clipboard(text: &str) -> Vec<Vec<String>> {
    let delim = if text.contains('\t') { '\t' } else { ',' };
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| line.split(delim).map(|cell| cell.trim().to_string()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kit_csv_test_{name}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_sniff_delimiter() {
        assert_eq!(sniff_delimiter(b"a;b;c\n1;2;3\n"), ';');
        assert_eq!(sniff_delimiter(b"a,b,c\n1,2,3\n"), ',');
        assert_eq!(sniff_delimiter(b"a\tb\tc\n1\t2\t3\n"), '\t');
        assert_eq!(sniff_delimiter(b"a|b|c\n1|2|3\n"), '|');
    }

    #[test]
    fn test_decode_bytes_encodings() {
        // UTF-8
        let (txt, enc) = decode_bytes("Hallo äöüß".as_bytes(), None);
        assert_eq!(txt, "Hallo äöüß");
        assert_eq!(enc, "utf-8");

        // UTF-8 BOM
        let mut bom_bytes = vec![0xEF, 0xBB, 0xBF];
        bom_bytes.extend_from_slice("Name;Wert\n".as_bytes());
        let (txt, enc) = decode_bytes(&bom_bytes, None);
        assert_eq!(txt, "Name;Wert\n");
        assert_eq!(enc, "utf-8-bom");

        // Windows-1252 mit äöüß (0xE4 0xF6 0xFC 0xDF)
        let win1252_bytes = b"Gr\xfc\xdfe \xe4\xf6\xfc\xdf";
        let (txt, enc) = decode_bytes(win1252_bytes, None);
        assert_eq!(txt, "Grüße äöüß");
        assert_eq!(enc, "windows-1252");
    }

    #[test]
    fn test_read_text_auto_and_header_map() {
        let dir = unique_temp_dir("auto");
        let file = dir.join("win1252.txt");
        std::fs::write(&file, b"Stra\xdfe;H\xe4user\n").unwrap();

        let (text, enc) = read_text_auto(&file, None).unwrap();
        assert_eq!(text, "Straße;Häuser\n");
        assert_eq!(enc, "windows-1252");

        let map = build_header_map(&[" Name ".into(), "PORT".into(), "status".into()]);
        assert_eq!(map.get("name"), Some(&0));
        assert_eq!(map.get("port"), Some(&1));
        assert_eq!(map.get("status"), Some(&2));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_and_read_csv_normalized_with_backup() {
        let dir = unique_temp_dir("save");
        let file = dir.join("export.csv");

        let headers = vec!["Name".to_string(), "Ort".to_string()];
        let rows_v1 = vec![
            vec!["Alice".to_string(), "München".to_string()],
            vec!["".to_string(), "   ".to_string()], // leere Zeile
            vec!["Bob;Jr".to_string(), "Köln".to_string()],
        ];

        save_csv_normalized(
            &file,
            &headers,
            &rows_v1,
            ';',
            CsvEncoding::Windows1252,
            true,
            true,
        )
        .unwrap();

        let parsed = read_csv_records(&file, None, None).unwrap();
        assert_eq!(parsed.delimiter, ';');
        assert_eq!(parsed.encoding, CsvEncoding::Windows1252);
        assert_eq!(parsed.headers, headers);
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0], vec!["Alice", "München"]);
        assert_eq!(parsed.rows[1], vec!["Bob;Jr", "Köln"]);

        // Zweites Speichern mit create_bak=true legt export.csv.bak an
        let rows_v2 = vec![vec!["Charlie".to_string(), "Berlin".to_string()]];
        save_csv_normalized(&file, &headers, &rows_v2, ';', CsvEncoding::Utf8Bom, true, true)
            .unwrap();

        let bak_file = dir.join("export.csv.bak");
        assert!(bak_file.exists());
        let parsed_v2 = read_csv_records(&file, None, None).unwrap();
        assert_eq!(parsed_v2.encoding, CsvEncoding::Utf8Bom);
        assert_eq!(parsed_v2.rows, rows_v2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_analyze_column_suggestions() {
        let rows = vec![
            vec!["Switch".into(), "Cisco".into()],
            vec!["Router".into(), "Cisco".into()],
            vec!["Switch".into(), "Aruba".into()],
            vec!["Switch".into(), " ".into()],
            vec!["AP".into(), "Cisco".into()],
        ];

        let all = analyze_column_suggestions(&rows, 0, "", 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].value, "Switch");
        assert_eq!(all[0].count, 3);
        assert_eq!(all[0].display_label, "Switch (3)");
        assert_eq!(all[0].count_text, "3×");

        let filtered = analyze_column_suggestions(&rows, 0, "swi", 10);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "Switch");

        let limited = analyze_column_suggestions(&rows, 1, "", 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].value, "Cisco");
    }

    #[test]
    fn test_tsv_clipboard_roundtrip() {
        let rows = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let tsv = to_tsv_clipboard(&rows);
        assert_eq!(tsv, "A\tB\n1\t2");
        assert_eq!(from_tsv_clipboard(&tsv), rows);
    }
}
