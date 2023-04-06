use crate::reader::record::GbamRecord;
use crate::reader::reader::Reader;
use crate::reader::records::Records;
use bitflags::bitflags;
use std::fmt;
use std::str;
use std::io::Write;
use std::string::String;
use std::time::Instant;
use bam_tools::record::fields::Fields;
use crate::reader::parse_tmplt::ParsingTemplate;

// https://github.com/samtools/htslib/blob/32de287eafdafc45dde0a22244b72697294f161d/htslib/sam.h
bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        struct BamFlags: u32 {
            /// @abstract the read is paired in sequencing, no matter whether it is mapped in a pair.
            const BAM_FPAIRED =        1;
            /// @abstract the read is mapped in a proper pair.
            const BAM_FPROPER_PAIR =   2;
            /// @abstract the read itself is unmapped; conflictive with BAM_FPROPER_PAIR.
            const BAM_FUNMAP =         4;
            /// @abstract the mate is unmapped.
            const BAM_FMUNMAP =        8;
            /// @abstract the read is mapped to the reverse strand.
            const BAM_FREVERSE =      16;
            /// @abstract the mate is mapped to the reverse strand.
            const BAM_FMREVERSE =     32;
            /// @abstract this is read1.
            const BAM_FREAD1 =        64;
            /// @abstract this is read2.
            const BAM_FREAD2 =       128;
            /// @abstract not primary alignment.
            const BAM_FSECONDARY =   256;
            /// @abstract QC failure.
            const BAM_FQCFAIL =      512;
            /// @abstract optical or PCR duplicate.
            const BAM_FDUP =        1024;
            /// @abstract supplementary alignment.
            const BAM_FSUPPLEMENTARY = 2048;
    }
}

#[derive(Default)]
struct Stats {
    pub n_reads: [i64; 2],
    pub n_mapped: [i64; 2],
    pub n_pair_all: [i64; 2],
    pub n_pair_map: [i64; 2],
    pub n_pair_good: [i64; 2],
    pub n_sgltn: [i64; 2],
    pub n_read1: [i64; 2],
    pub n_read2: [i64; 2],
    pub n_dup: [i64; 2],
    pub n_diffchr: [i64; 2],
    pub n_diffhigh: [i64; 2],
    pub n_secondary: [i64; 2],
    pub n_supp: [i64; 2],
    pub n_primary: [i64; 2],
    pub n_pmapped: [i64; 2],
    pub n_pdup: [i64; 2],
}

fn percent(n: i64, total: i64) -> String
{   

    if total != 0 {
        // dbg!(n, total, ((n as f64) / ((total as f64) * 100.0)));
        format!("{:.2}%", (n as f64) / ((total as f64)) * 100.0)
    } 
    else {
       String::from("N/A")
    }

}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {        
        writeln!(f, "{} + {} in total (QC-passed reads + QC-failed reads)", self.n_reads[0], self.n_reads[1]).unwrap();
        writeln!(f, "{} + {} primary", self.n_primary[0], self.n_primary[1]).unwrap();
        writeln!(f, "{} + {} secondary", self.n_secondary[0], self.n_secondary[1]).unwrap();
        writeln!(f, "{} + {} supplementary", self.n_supp[0], self.n_supp[1]).unwrap();
        writeln!(f, "{} + {} duplicates", self.n_dup[0], self.n_dup[1]).unwrap();
        writeln!(f, "{} + {} primary duplicates", self.n_pdup[0], self.n_pdup[1]).unwrap();
        writeln!(f, "{} + {} mapped ({} : {})", self.n_mapped[0], self.n_mapped[1], percent(self.n_mapped[0], self.n_reads[0]), percent(self.n_mapped[1], self.n_reads[1])).unwrap();
        writeln!(f, "{} + {} primary mapped ({} : {})", self.n_pmapped[0], self.n_pmapped[1], percent(self.n_pmapped[0], self.n_primary[0]), percent(self.n_pmapped[1], self.n_primary[1])).unwrap();
        writeln!(f, "{} + {} paired in sequencing", self.n_pair_all[0], self.n_pair_all[1]).unwrap();
        writeln!(f, "{} + {} read1", self.n_read1[0], self.n_read1[1]).unwrap();
        writeln!(f, "{} + {} read2", self.n_read2[0], self.n_read2[1]).unwrap();
        writeln!(f, "{} + {} properly paired ({} : {})", self.n_pair_good[0], self.n_pair_good[1], percent(self.n_pair_good[0], self.n_pair_all[0]), percent(self.n_pair_good[1], self.n_pair_all[1])).unwrap();
        writeln!(f, "{} + {} with itself and mate mapped", self.n_pair_map[0], self.n_pair_map[1]).unwrap();
        writeln!(f, "{} + {} singletons ({} : {})", self.n_sgltn[0], self.n_sgltn[1], percent(self.n_sgltn[0], self.n_pair_all[0]), percent(self.n_sgltn[1], self.n_pair_all[1])).unwrap();
        writeln!(f, "{} + {} with mate mapped to a different chr", self.n_diffchr[0], self.n_diffchr[1]).unwrap();
        write!(f, "{} + {} with mate mapped to a different chr (mapQ>=5)", self.n_diffhigh[0], self.n_diffhigh[1])
        
    }
}

fn collect(rec: &Bundle, stats: &mut Stats) {
    let record_flag = BamFlags::from_bits(rec.flag as u32).unwrap();
    let w = record_flag.contains(BamFlags::BAM_FQCFAIL) as usize;
    
    stats.n_reads[w] += 1;

    if record_flag.contains(BamFlags::BAM_FSECONDARY) {
        stats.n_secondary[w] += 1;
    }
    else if record_flag.contains(BamFlags::BAM_FSUPPLEMENTARY) {
        stats.n_supp[w] += 1;
    } else {
        stats.n_primary[w] += 1;

        if record_flag.contains(BamFlags::BAM_FPAIRED) {
            stats.n_pair_all[w] += 1;
            if record_flag.contains(BamFlags::BAM_FPROPER_PAIR) && !record_flag.contains(BamFlags::BAM_FUNMAP) {
                stats.n_pair_good[w] += 1;
            }
            if record_flag.contains(BamFlags::BAM_FREAD1) {
                stats.n_read1[w] += 1;
            }
            if record_flag.contains(BamFlags::BAM_FREAD2) {
                stats.n_read2[w] += 1;
            }
            if record_flag.contains(BamFlags::BAM_FMUNMAP) &&  !record_flag.contains(BamFlags::BAM_FUNMAP){
                stats.n_sgltn[w] += 1;
            }
            if !record_flag.contains(BamFlags::BAM_FUNMAP) &&  !record_flag.contains(BamFlags::BAM_FMUNMAP){
                stats.n_pair_map[w] += 1;
                if rec.next_ref_id != rec.refid {
                    stats.n_diffchr[w] += 1;
                    if rec.mapq >= 5 {
                        stats.n_diffhigh[w] += 1;
                    }
                }
            }
        }
        if !record_flag.contains(BamFlags::BAM_FUNMAP) {
            stats.n_pmapped[w] += 1;
        }
        if record_flag.contains(BamFlags::BAM_FDUP) {
            stats.n_pdup[w] += 1;
        }
        
    }
    if !record_flag.contains(BamFlags::BAM_FUNMAP) {
        stats.n_mapped[w] += 1;
    }
    if record_flag.contains(BamFlags::BAM_FDUP) {
        stats.n_dup[w] += 1;
    }

    // int w = (c->flag & BAM_FQCFAIL)? 1 : 0;
    // ++s->n_reads[w];
    // if (c->flag & BAM_FSECONDARY ) {
    //     ++s->n_secondary[w];
    // } else if (c->flag & BAM_FSUPPLEMENTARY ) {
    //     ++s->n_supp[w];
    // } else {
    //     ++s->n_primary[w];

    //     if (c->flag & BAM_FPAIRED) {
    //         ++s->n_pair_all[w];
    //         if ((c->flag & BAM_FPROPER_PAIR) && !(c->flag & BAM_FUNMAP) ) ++s->n_pair_good[w];
    //         if (c->flag & BAM_FREAD1) ++s->n_read1[w];
    //         if (c->flag & BAM_FREAD2) ++s->n_read2[w];
    //         if ((c->flag & BAM_FMUNMAP) && !(c->flag & BAM_FUNMAP)) ++s->n_sgltn[w];
    //         if (!(c->flag & BAM_FUNMAP) && !(c->flag & BAM_FMUNMAP)) {
    //             ++s->n_pair_map[w];
    //             if (c->mtid != c->tid) {
    //                 ++s->n_diffchr[w];
    //                 if (c->qual >= 5) ++s->n_diffhigh[w];
    //             }
    //         }
    //     }

    //     if (!(c->flag & BAM_FUNMAP)) ++s->n_pmapped[w];
    //     if (c->flag & BAM_FDUP) ++s->n_pdup[w];
    // }
    // if (!(c->flag & BAM_FUNMAP)) ++s->n_mapped[w];
    // if (c->flag & BAM_FDUP) ++s->n_dup[w];
}

#[derive(Default, Clone, Copy)]
#[repr(C)] 
struct Bundle {
    refid: i32,
    next_ref_id:i32,
    flag: u16,
    mapq: u8,
}

static mut uncompress_time : u128 =  0;



pub fn collect_stats(reader: &mut Reader) {
    let mut stats = Stats::default();
    let mut buf =  GbamRecord::default();

    const BUF_SIZE: usize = 1_000_000;
    let mut recs = vec![Bundle::default(); BUF_SIZE];
    // dbg!("WHAT");
    let mut tmplt = ParsingTemplate::new();
    let mut current_record = 0;
    
    loop {
        // dbg!(current_record);
        if current_record == reader.amount {
            break;
        }
        let available_records = std::cmp::min(BUF_SIZE, reader.amount-current_record);
        
        let column = reader.get_column(&Fields::RefID);
        for offset in 0..available_records {
            column.fill_record_field(current_record+offset, &mut buf);
            if buf.refid.is_none() {
                dbg!(current_record+offset);
            }
            // recs[offset].refid = buf.refid.unwrap();
        }
        
        let column = reader.get_column(&Fields::NextRefID);
        for offset in 0..available_records {
            column.fill_record_field(current_record+offset, &mut buf);
            // recs[offset].next_ref_id = buf.next_ref_id.unwrap();
        }
        
        let column = reader.get_column(&Fields::Flags);
        for offset in 0..available_records {
            column.fill_record_field(current_record+offset, &mut buf);
            // recs[offset].flag = buf.flag.unwrap();
        }
        let now = Instant::now();
        let column = reader.get_column(&Fields::Mapq);
        for offset in 0..available_records {
            column.fill_record_field(current_record+offset, &mut buf);
            // recs[offset].mapq = buf.mapq.unwrap();
        }
        unsafe {
            uncompress_time += now.elapsed().as_micros();
        }

        
        for offset in 0..available_records {
            // collect(&recs[offset], &mut stats);
        }
        
        current_record += available_records;
        
    }
    unsafe {
    dbg!(uncompress_time/1000);
    
    }
    // tmplt.set(&Fields::RefID, true);
    // tmplt.set(&Fields::NextRefID, true);
    // tmplt.set(&Fields::Mapq, true);


    
    // let mut count = 0;
    // while let Some(rec) = records.next_rec() {
    //     recs[count].refid = rec.refid.unwrap();
    //     recs[count].nextrefid = rec.next_ref_id.unwrap();
    //     recs[count]. = rec.refid.unwrap();
    //     recs[count].refid = rec.refid.unwrap();
    //     collect(rec, &mut stats);
    // }
    println!("{stats}");
}
