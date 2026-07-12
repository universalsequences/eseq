I ran Instruments profiler while using process-project-performance-lanes-demo.lisp because it was lagging, and found that we're doing an incredible amount of work in the sequencer thread due to all the processes (which are very light weight btw).

I want you to 
1. create a profile that is able to measure how long it takes to invoke_process_cascade on a project with 4 tracks AND the same process-project-perfoormance-lanes-demo.lisp loaded in it (i.e. processes attached), with 1/16 triggers on each track running through them 
2. use "goal" mode to reduce that time by 10x

Here is the instruments profile dump:

Weight	Self Weight	Symbol Names
2.08 s  38.0%	0 s	                sequencer::scheduler::schedule_playing_lookahead::h5ae93ebe3b934ef9
2.04 s  37.2%	0 s	                 sequencer::scheduler::invoke_process_cascade::h5b192087dc9d3dfb
2.03 s  37.1%	0 s	                  sequencer::lisp_host::ScratchControlRuntime::invoke_process_run::heccc1e9e103da7f2
2.03 s  37.1%	0 s	                   eseqlisp::runtime::Runtime::eval_str::hb8f5d6c57383ba1c
2.03 s  37.1%	1.00 ms	                    eseqlisp::lang::vm::VM::eval_str::h5e519666a59595a2
1.21 s  22.1%	0 s	                     _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::h580084b2441b872e
1.21 s  22.1%	0 s	                      alloc::slice::_$LT$impl$u20$$u5b$T$u5d$$GT$::to_vec_in::h281f78b18ecab336
1.21 s  22.1%	11.00 ms	                       _$LT$T$u20$as$u20$alloc..slice..$LT$impl$u20$$u5b$T$u5d$$GT$..to_vec_in..ConvertVec$GT$::to_vec::hf069d89cb58de92f
1.18 s  21.6%	25.00 ms	                        _$LT$eseqlisp..lang..compiler..Chunk$u20$as$u20$core..clone..Clone$GT$::clone::h60e9b2c313d4f0e0
569.00 ms  10.4%	6.00 ms	                         _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::h257d0f351773dc76
553.00 ms  10.1%	0 s	                          alloc::slice::_$LT$impl$u20$$u5b$T$u5d$$GT$::to_vec_in::h38ab7915645e88e0
553.00 ms  10.1%	3.00 ms	                           _$LT$T$u20$as$u20$alloc..slice..$LT$impl$u20$$u5b$T$u5d$$GT$..to_vec_in..ConvertVec$GT$::to_vec::haf604540308ce5f8
383.00 ms   7.0%	8.00 ms	                            _$LT$alloc..string..String$u20$as$u20$core..clone..Clone$GT$::clone::h390876fd411fb394
375.00 ms   6.9%	0 s	                             _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::h62ebea5428bd3159
375.00 ms   6.9%	0 s	                              alloc::slice::_$LT$impl$u20$$u5b$T$u5d$$GT$::to_vec_in::h8c35045fc3727415
375.00 ms   6.9%	0 s	                               _$LT$T$u20$as$u20$alloc..slice..$LT$impl$u20$$u5b$T$u5d$$GT$..to_vec_in..ConvertVec$GT$::to_vec::h83100ae968dfc210
293.00 ms   5.4%	0 s	                                alloc::vec::Vec$LT$T$C$A$GT$::with_capacity_in::h9636d7a87141c87b
82.00 ms   1.5%	0 s	                                core::ptr::const_ptr::_$LT$impl$u20$$BP$const$u20$T$GT$::copy_to_nonoverlapping::h28c54cdca5f30409
125.00 ms   2.3%	0 s	                            alloc::vec::Vec$LT$T$C$A$GT$::with_capacity_in::hfa54f9d721a70e37
23.00 ms   0.4%	23.00 ms	                            core::mem::maybe_uninit::MaybeUninit$LT$T$GT$::write::hbc63f9d5c4c84add
19.00 ms   0.3%	14.00 ms	                            _$LT$core..iter..adapters..take..Take$LT$I$GT$$u20$as$u20$core..iter..traits..iterator..Iterator$GT$::next::h78f4fb10d36e5328
10.00 ms   0.2%	0 s	                          _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..ops..deref..Deref$GT$::deref::hb5b4ebfe61b68585
10.00 ms   0.2%	10.00 ms	                           alloc::vec::Vec$LT$T$C$A$GT$::as_slice::h1eb06f490dc9d42e
546.00 ms  10.0%	0 s	                         _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::h9fe22301235fa869
546.00 ms  10.0%	0 s	                          alloc::slice::_$LT$impl$u20$$u5b$T$u5d$$GT$::to_vec_in::hac63c04ea8ba8a4d
546.00 ms  10.0%	2.00 ms	                           _$LT$T$u20$as$u20$alloc..slice..$LT$impl$u20$$u5b$T$u5d$$GT$..to_vec_in..ConvertVec$GT$::to_vec::h10c02573456e264c
250.00 ms   4.6%	250.00 ms	                            core::mem::maybe_uninit::MaybeUninit$LT$T$GT$::write::h3082e7eb97c28e41
139.00 ms   2.5%	139.00 ms	                            _$LT$eseqlisp..lang..compiler..OpCode$u20$as$u20$core..clone..Clone$GT$::clone::h6cddcbd1088ee546
133.00 ms   2.4%	0 s	                            alloc::vec::Vec$LT$T$C$A$GT$::with_capacity_in::h6551633b0f72aa13
133.00 ms   2.4%	0 s	                             alloc::raw_vec::RawVec$LT$T$C$A$GT$::with_capacity_in::h2efb549f1004a55a
22.00 ms   0.4%	10.00 ms	                            _$LT$core..iter..adapters..take..Take$LT$I$GT$$u20$as$u20$core..iter..traits..iterator..Iterator$GT$::next::h540a6e79349306ff
34.00 ms   0.6%	0 s	                         _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::hd76f46d954591cb4
34.00 ms   0.6%	0 s	                          alloc::slice::_$LT$impl$u20$$u5b$T$u5d$$GT$::to_vec_in::heb8ed19e7175ed82
8.00 ms   0.1%	8.00 ms	                         _$LT$alloc..string..String$u20$as$u20$core..clone..Clone$GT$::clone::h390876fd411fb394
1.00 ms   0.0%	1.00 ms	                         _$LT$core..option..Option$LT$T$GT$$u20$as$u20$core..clone..Clone$GT$::clone::hb9dfa97f9ecc297f
1.00 ms   0.0%	1.00 ms	                         _$LT$core..option..Option$LT$T$GT$$u20$as$u20$core..clone..Clone$GT$::clone::hca9e8a68c1471f72
13.00 ms   0.2%	13.00 ms	                        core::mem::maybe_uninit::MaybeUninit$LT$T$GT$::write::h090f4a977794b042
1.00 ms   0.0%	0 s	                        _$LT$core..iter..adapters..take..Take$LT$I$GT$$u20$as$u20$core..iter..traits..iterator..Iterator$GT$::next::hbdcc82f60049898b
1.00 ms   0.0%	0 s	                        alloc::vec::Vec$LT$T$C$A$GT$::with_capacity_in::he730c408ceba5bda
776.00 ms  14.2%	0 s	                     core::ptr::drop_in_place$LT$alloc..vec..Vec$LT$eseqlisp..lang..compiler..Chunk$GT$$GT$::hc489ddf5fdf1bd42
776.00 ms  14.2%	0 s	                      _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..ops..drop..Drop$GT$::drop::h59726c5ab56e47f9
776.00 ms  14.2%	3.00 ms	                       core::ptr::drop_in_place$LT$$u5b$eseqlisp..lang..compiler..Chunk$u5d$$GT$::h2d6bec8f2e8ba1ef
744.00 ms  13.6%	0 s	                        core::ptr::drop_in_place$LT$eseqlisp..lang..compiler..Chunk$GT$::h3908141f54f40385
561.00 ms  10.2%	0 s	                         core::ptr::drop_in_place$LT$alloc..vec..Vec$LT$alloc..string..String$GT$$GT$::hb4601d41beee2da7
376.00 ms   6.9%	0 s	                          _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..ops..drop..Drop$GT$::drop::hadf0b4553db29ad2
369.00 ms   6.7%	9.00 ms	                           core::ptr::drop_in_place$LT$$u5b$alloc..string..String$u5d$$GT$::hcc4abb64f9ba47a4
7.00 ms   0.1%	0 s	                           alloc::vec::Vec$LT$T$C$A$GT$::as_mut_ptr::hfbd57c584dfd3396
185.00 ms   3.4%	0 s	                          core::ptr::drop_in_place$LT$alloc..raw_vec..RawVec$LT$alloc..string..String$GT$$GT$::ha4906d2ab421b4d4
161.00 ms   2.9%	0 s	                         core::ptr::drop_in_place$LT$alloc..vec..Vec$LT$eseqlisp..lang..compiler..OpCode$GT$$GT$::h20df5d30edeb382a
19.00 ms   0.3%	0 s	                         core::ptr::drop_in_place$LT$alloc..vec..Vec$LT$f64$GT$$GT$::h65dfed134bb0ef98
2.00 ms   0.0%	2.00 ms	                         core::ptr::drop_in_place$LT$core..option..Option$LT$std..path..PathBuf$GT$$GT$::hca1d78e0cbd5777b
1.00 ms   0.0%	1.00 ms	                         core::ptr::drop_in_place$LT$core..option..Option$LT$alloc..string..String$GT$$GT$::h6e3c5bfa54170575
29.00 ms   0.5%	29.00 ms	                        _xzm_free
13.00 ms   0.2%	13.00 ms	                     _$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..clone..Clone$GT$::clone::h257d0f351773dc76
