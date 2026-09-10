;; PM Ride / reduced dispersive plate, identified from Donit's cymbal recordings.
;; Six frequency regions, each with three paths and an orthogonal junction.
;; Eight resolved resonances carry the bell/foot modes; dense plate modes live
;; in the delay network. No PCM, recorded phase or sampled envelope is used.
;; Rebuild and validation: tools/pm-cymbals/README.md.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
(param character @group voicing @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
(param size @group body @default 1 @min 0.5 @max 2 @mod true @mod-mode additive)
(param decay @group body @default 1 @min 0.2 @max 3 @mod true @mod-mode additive)
(param damping @group body @default 1 @min 0.25 @max 3 @mod true @mod-mode additive)
(param hardness @group stick @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
(param bell @group body @default 0.25 @min 0 @max 2 @mod true @mod-mode additive)
(param wash @group body @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param touch @group contact @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param color @group output @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param width @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @group output @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param tracking @group tuning @default 0 @min 0 @max 1)
;; Calibration SHA256: add792bf642347aeab5f80b0ad7ead93c39c8807367a0bb910b321e5efccfad9
(def voice_material (tensor @shape [232] @data [
  10.63748738 1.86933042 6.788523197 2.888911486 0.8488690746 9.581352651 0.008 0.3
  1.387513506 8.738331441 0.1039831445 2.352402615 5.775360263 16.7348113 0.0006 0
  0.8330055882 1.597339665 1.308762372 0.817309996 3.069078198 8.094439113 0.0006 0
  0.1131375796 9.858320753 2.004468625 4.286853243 5.758651882 5.931462129 0.008 0
  0.207073638 1.024883565 4.350311404 3.460799127 5.458475053 13.91290574 0.0006 0
  0.1673094101 1.265932062 0.2625319409 0.1506870295 3.326204792 8.461567565 0.0006 0
  0.29587913 0.6482479163 0.6888591084 1.112039708 3.875075288 20.7932758 0.008 0
  4.746687183 0.1 0.1000000009 1.046192689 1.869448887 14.96029127 0.008 0
  0.1519191132 0.6838809131 0.5134685497 0.8658117853 4.679428234 13.97834237 0.008 0
  0.3941004026 1.183816087 1.154358513 1.399984098 5.023522533 1.3847736 0.008 0
  0.1869564161 2.748956868 1.4275263 1.506565292 2.588380778 9.360794411 0.0006 0
  1.75332151 0.5744032278 0.46303692 0.6679919648 3.257860234 15.22888666 0.0006 1
  0.1802906282 1.656683882 0.5015387089 0.6333789169 2.490102831 10.34553823 0.0006 0
  5.806264568 0.3140940701 1.760315957 0.8539498804 2.378770707 8.560864444 0.0006 1
  8.460355321 3.192813236 0.7549250754 1.03735128 2.77164844 9.061183222 0.0006 1
  0.6935377852 9.278587964 1.137343913 1.038937745 2.184592203 10.70372043 0.0006 0
  0.4436998846 4.326438798 1.938957502 1.774213615 2.61130943 12.47083413 0.0006 0
  1.702624567 1.246677673 1.301652233 5.341051438 4.392974689 11.58593249 0.0006 0.3
  0.1000000007 4.706085476 0.565224265 2.854175704 3.603971251 17.22653039 0.008 0
  8.612594895 0.1000008163 0.7190931467 9.58375837 1.506811355 10.0234105 0.0025 0.3
  0.1000000189 0.100000262 0.898064342 1.032225915 1.958208496 6.296726237 0.008 0
  0.2025799679 0.4456908713 0.9178357403 10.84839516 1.181594414 16.62327344 0.0006 0
  0.1000004988 0.2084739506 3.963025561 2.101628284 3.677282266 5.545585854 0.0006 0
  2.414097906 0.2923387733 0.9705444525 2.163576278 2.40983599 9.628612287 0.008 0
  0.1143539268 1.23978138 0.6441232566 0.8630594678 1.658247216 4.485220336 0.0006 1
  0.1000000002 0.9561458179 1.315206035 1.046463915 1.264560566 24.47757012 0.0006 0
  0.6218927492 0.1000000015 1.626504787 6.077126842 0.656597016 8.842304159 0.0006 0
  0.3648911445 0.497570436 0.5062856195 0.814262205 1.267256428 5.128353183 0.0006 0
  2.689460571 0.7570371827 5.411137348 5.232259316 3.297894318 7.769548418 0.0006 1
]))
(def voice_band_gains (tensor @shape [522] @data [
  1.265172272e-16 1.871842725e-16 2.751883171e-16 2.549782251e-16 1.846122321e-17 2.494261756 2.726261777e-17 6.535066184 1.483442562 1.409450932e-05 1.220564789 0.03313542108 0.1746313057 9.704809007e-16 1.541134565e-16 0.03835146637 0.1858196865 1.220688847e-15
  1.14312547 1.048247732e-10 0.05881311394 9.694861385e-11 0.96669428 1.110083312 0.02120729001 0.03156391849 0.1300900058 0.676425028 2.516585306 4.796284363 2.560663271e-10 8.939877295 12.79452062 26.81868838 7.652952389 8.080591869
  2.611326651 4.281948951e-09 0.3619434378 7.071249333e-09 0.2200222788 0.004953598105 8.117481813e-09 6.850003363e-09 0.7260903589 2.348943316e-09 9.619928238e-09 5.61921759 5.3972572e-06 17.88636139 9.643650203 28.62540717 15.97256864 10.60283332
  0.1098258519 3.231872304e-26 0.01798734462 0.1750812128 4.587749762e-25 2.03459405e-24 2.090626305e-27 1.694454176 3.365823663e-26 0.271332304 4.359283961 9.584535427e-23 9.286234195e-24 2.082410433 5.801429743e-20 4.590928938 2.987462922 1.364204746
  6.680411343e-09 0.1581563116 1.604108129e-09 4.156963069e-09 1.547156663 0.1997584737 7.258129655e-09 7.84210357e-09 8.562417407e-09 5.184374213 1.989979319 16.44316249 1.547859611e-08 18.4946955 26.7394591 43.2581004 26.11918266 23.93801842
  1.028538073 1.379818439e-35 0.3218073495 8.178025079e-31 1.15975169e-21 0.7167339067 0.349342311 0.04017609349 0.07398767724 8.270249646e-17 4.149830163e-18 11.59357185 1.003131059e-15 0.03152268508 28.10163815 45.59520855 50.77289208 35.26622047
  0.4743694362 4.224685022e-36 0.125602024 9.266961745e-35 1.074985625 0.4499971529 0.07179739675 4.438141478e-32 1.105881655 2.219160094e-30 1.233440811e-25 1.954009214 1.93289363 5.667831917 1.832224999e-16 20.59691624 12.32868984 2.916223542e-27
  3.879641961e-34 1.240425889e-33 5.924231774e-33 1.00949852e-29 1.071810119 2.317838817e-32 0.02540774125 2.797804291e-36 0.7266831637 7.223778952e-33 2.225661949 1.982109537e-30 6.246572397e-05 7.433365491 2.388602667e-23 13.59209901 1.736615417 2.011690803e-48
  0.4667090779 1.205607933e-38 0.1883382607 1.347371653e-32 0.2582140776 1.384844299 4.386662442e-37 2.339199745 8.386229599e-33 1.428372461 2.435184671 2.660026129e-35 2.442638962e-05 5.721839387 4.83899128 2.476953842e-19 22.21859328 0.07379422541
  0.6192699938 4.222905298e-11 0.08772096841 6.387456093e-11 1.483176773 9.841641392e-11 1.418062869e-10 2.947422987 1.241689579e-10 1.027457388e-10 8.319651612 1.08981198 11.00455788 9.383113245 4.393618199 1.642812925 2.39343295 1.790262
  0.797107146 8.535136713e-28 0.0529967556 2.536464436e-05 0.3855464446 6.020702155e-24 0.5781090584 4.314728048e-05 2.336939474 5.912810511e-20 1.852703023e-16 14.54961165 2.896355109e-11 2.608584625 33.01275934 42.84943988 37.39035264 23.61448334
  0.01472539023 3.236164484e-11 3.358008734e-11 1.564884646 2.076610176e-11 1.965342547e-11 1.34879504 1.816554424e-11 1.938423914e-11 2.444094491e-11 0.08850601679 7.846572743 5.011395933e-11 6.257571282e-11 16.19595774 1.240764062e-10 12.35260881 24.0367301
  4.376290102e-10 3.839198208e-10 7.206282749e-10 1.041262445e-09 1.337092314 6.571985073e-09 1.891619651 0.1945424665 0.7304897022 8.14791957e-09 2.554782884 4.633376086 2.769130221e-08 13.60090877 22.54739988 53.87249544 39.56529419 6.10855171
  5.486605667e-11 0.01574871618 0.4171791899 3.561860969e-11 1.654280497 2.786088865e-11 2.550997625e-11 1.699884136 2.750080924e-11 3.45980569e-11 5.240895914e-11 10.7273553 7.087366437e-11 8.756133261e-11 14.20414182 1.752472095e-10 22.07953973 3.081707537e-10
  5.420970737e-11 4.596580326e-11 4.779011216e-11 3.538252888e-11 2.131770936 1.285485788 1.341059498 2.544223575e-11 2.717547808e-11 3.427102317e-11 5.255572453e-11 6.986520522 7.020928842e-11 2.870521865 8.958038787 1.734240608e-10 17.60541689 3.061163914e-10
  1.682627743 7.073659629e-25 0.1773473253 2.254163121 1.204828414e-19 1.609194007e-16 0.09404026204 0.03006523157 9.912825259e-07 7.462822893e-15 4.081207658 3.045750105e-11 6.515887614e-15 1.168437685e-08 32.23245103 29.82393886 36.19415998 29.1832481
  2.230897513 9.948958393e-11 0.2324500178 8.913002466e-11 0.5079601124 1.77719494 7.625320188e-11 3.802244434 1.702505129 2.148340125 1.622911068e-10 18.09280529 2.463060617e-10 9.139495693 22.92225631 30.94152546 22.35702655 26.42595364
  6.888551373e-14 2.912727247e-12 1.127264521e-13 2.113105204e-17 1.961223477 2.096085934e-21 0.2522362669 2.548686771e-13 0.2602069314 0.5682632779 2.143570345e-11 0.3961831497 1.715219486e-12 21.46945638 0.0001090747624 56.06272756 2.546333592e-12 2.639061039
  0.334852697 4.243715139e-27 0.1276195551 0.05109682789 3.289823646e-16 2.033577911e-27 5.634956909e-27 1.191121737 4.542992539e-26 3.437821219 9.336424276 5.439494294e-27 4.969969658e-29 5.095438291 9.283088455 7.67581637 9.460025246 7.976421777
  6.274460742e-09 4.691273548e-09 0.01629175816 0.001951970668 5.96244365e-09 0.2845772025 1.580855189e-08 1.481659005 0.1546940095 1.471018767e-08 1.821235421e-08 2.349913709e-08 7.926309864 5.187179283 4.37815903 23.39221108 5.131384095e-08 9.606519883
  0.06013992056 2.329530638e-29 0.02075833487 4.168189377e-30 0.4763058143 3.110406635e-29 1.205730473 2.480341433e-23 0.2986460991 1.335474048e-25 1.061787025 0.3941758612 0.4130137752 2.817286645 5.072238386 7.635368704 13.16624048 8.285793245e-15
  0.9483240247 1.012497384e-10 0.05770090452 8.436822838e-11 0.1875065485 0.07904179679 1.099531547 6.982987746e-11 0.6105966684 1.148941192e-10 1.678835856e-10 23.17261392 3.005761547 2.933467632e-10 15.16325258 114.986707 75.59382507 39.24515161
  0.4052658014 3.398793538e-11 6.499215575e-11 1.69830341e-11 4.819487998e-11 0.2216043377 1.606200504 5.514481719e-11 0.9903534774 0.03916112561 1.251936201e-10 1.587622851e-10 1.629704484e-10 2.202993893e-10 3.215198855e-10 61.80253075 28.77457027 8.418068445e-10
  2.646824478e-33 3.178710227e-35 0.2378646289 1.79560718e-27 5.661593436e-21 0.7340744986 1.493693884e-32 3.50139176 1.952634838e-33 0.5221592483 2.06186015e-31 5.909070305 0.3711408411 10.15993516 14.33172038 7.845262589e-15 33.0122841 29.46566452
  0.4455763398 0.09818993763 0.5177013787 1.150728148e-10 2.152934521 2.135833986 0.8156913843 7.354185383e-10 0.02605254315 3.823535772e-10 1.119811753e-09 7.658885036 4.871121279 1.711984147e-09 23.78654402 6.698409404e-10 29.33024489 4.379368013e-09
  2.682499421 1.838938775e-29 0.1888662252 2.131798122e-28 2.528792506e-23 0.410248994 9.7673436e-30 9.916198436e-28 2.276456795e-28 1.941894063e-26 2.378265775e-27 7.789095184 2.649843151e-25 5.772857664 29.90521026 1.588876906e-12 203.790351 1.457176522e-24
  2.056086957 1.593602379e-10 1.221848989e-10 1.348155377e-10 1.584508015e-10 0.8428642414 3.298672148 6.946100086e-11 0.8781142391 0.5140004792 2.682523807e-10 3.485816744e-10 8.444951827 11.84578652 6.7018355e-10 1.019650069e-09 125.3395587 14.01007414
  0.6052504408 0.1077081237 0.7235101221 1.070826844e-18 2.646308213e-10 1.307068903 0.1561662608 0.1993455006 0.0002703652241 2.032518706e-16 2.209402775e-18 7.604681678 2.645693343e-13 4.382212142e-09 42.06730141 1.499786343e-12 66.38904302 0.2091218805
  0.1048935091 8.852484276e-09 0.1178273299 0.04814402314 1.033959122 4.460829168e-09 0.514849658 1.013978194 4.957494939e-09 0.002655013826 1.016925569e-08 0.8163829963 1.262193517e-08 10.64147716 0.4770174654 3.307699026e-08 29.0356139 5.405623252e-08
]))
(def voice_mode_frequencies (tensor @shape [232] @data [
  132.4483224 225.422248 275.6511617 323.8137279 349.3814773 422.3973245 449.7252223 607.3533686
  666.7508917 689.5422998 708.3217309 747.7932515 1687.48711 1719.115999 2612.722957 3659.160591
  865.6335011 895.190123 968.5549422 1079.353265 1100.984975 1116.936959 1843.79447 2637.229846
  364.7411352 473.8043733 488.5057976 528.3471139 563.0134844 587.6748591 1160.911928 2496.487144
  279.3996637 653.2209006 700.3417668 824.9497598 853.7199906 1000.604481 1015.083037 4181.041289
  314.2664671 1426.140214 1637.501424 2401.00455 2556.299088 2722.003914 2863.518948 3867.163511
  409.9889273 469.0511502 847.5080517 1377.170523 1548.612811 1575.067258 2657.583479 3889.362037
  857.8002271 1018.802116 1398.862786 2869.413821 3182.205065 3556.683146 3631.056658 5958.444257
  362.6537014 756.2735805 846.3602398 1422.579589 1490.800382 1638.638898 3843.099788 3963.956032
  686.6050675 980.2760128 1030.07293 2637.863722 2686.86622 3464.999334 4045.270921 4114.674646
  214.0302686 504.5951505 1017.153081 1497.196025 1549.541549 1665.824096 1819.326995 2416.900764
  386.8579765 705.6677401 725.1564926 1570.127716 2434.146389 2512.980445 2805.349652 3773.989124
  261.854072 323.5223936 525.6183652 2597.392608 2755.870914 3597.057149 3643.65192 3795.860878
  425.0773314 471.3292567 668.1621766 1395.002424 1488.183838 1615.623448 1647.160525 2929.192058
  1317.851588 1363.785703 1450.747081 2335.069221 2593.908763 2733.420735 3648.727783 4012.520063
  1776.339808 2012.945059 2470.370089 2754.102672 3301.151037 4105.370094 5324.063496 6160.644976
  413.1166278 1781.997648 2622.021109 3307.210169 3788.70879 4360.398555 5076.371316 5704.497134
  559.272616 625.3126094 3350.475293 3419.749536 3647.053085 4388.558226 4448.385324 10747.62583
  1204.166237 1306.715494 1978.007446 2495.377734 3034.954228 3273.198345 7002.59724 9340.54929
  1557.409724 1577.470659 2415.020264 2861.094415 3131.394632 3810.972031 3926.635708 5445.902929
  682.1657829 824.9440823 2449.213096 2501.251158 3958.840315 5382.559108 6440.092537 8295.125162
  2025.058755 4332.925855 4473.496621 4828.13135 5709.219015 6327.927011 6628.72149 8100.21343
  1846.638916 2940.71129 3986.371482 5176.612517 5236.528794 5339.359428 6219.960606 7917.826309
  506.8217394 593.0237538 611.4940153 5789.592564 9240.579061 10105.64458 10551.13135 12224.42045
  4645.216868 4729.915716 4810.388764 5962.235622 6134.204422 6435.505755 7233.216764 8015.089823
  881.4282145 4782.354478 5937.607577 6080.45494 7160.427244 7525.445653 8705.875719 9229.517521
  2030.284486 5431.524633 5494.127289 6587.858681 6811.629259 8256.386449 8443.053863 8633.544133
  378.4897574 439.847629 546.1325167 4441.100032 4573.821531 8652.854515 9019.636736 10025.19252
  1290.127502 3419.617544 7149.414576 7411.434521 7945.59585 8513.673999 9272.7691 10747.13746
]))
(def voice_mode_rates (tensor @shape [232] @data [
  2.222603057 2.718679248 3.886583782 3.29212825 3.679785789 3.270928832 3.84367105 2.112179074
  2.652202848 2.563597084 2.373048415 1.912210928 2.526964992 2.182666739 4.113342661 5.581631369
  1.326269289 1.443331338 1.016444495 0.9681863109 1.051551072 0.9840920521 1.178623274 1.14436393
  2.322156523 2.898124765 3.29451951 1.767743292 2.795976744 3.497496906 3.704805846 2.420966369
  2.086794266 2.527385823 2.055610959 2.086051965 2.162749615 1.867868834 2.347887007 4.246859879
  0.7419140068 0.9513509851 0.4186417504 1.453138197 1.315534607 1.320854438 1.273882114 2.310687411
  0.164036288 0.726312122 0.5808330389 1.225825761 0.7224743163 0.5755072403 1.75810746 2.154594771
  0.519377373 0.8212569747 1.063566952 3.223972281 1.273661488 2.858545008 1.054727754 1.185509258
  0.6591948447 0.4891164571 0.8739099771 0.8375790077 0.5492922204 0.8425341067 1.728693004 1.77930422
  1.665948331 1.070294698 0.8733999502 1.879223227 1.74905448 2.749702217 2.524990813 3.406000986
  1.242876201 1.566976342 1.590959548 1.04785247 1.855831408 1.878343969 1.561508517 1.296094855
  1.703139901 0.6965103499 0.1704580421 0.7002982819 1.482618342 2.004311974 1.420624912 1.185752791
  0.694442008 0.5014303713 0.7656109138 0.8882024061 1.201423573 2.816479683 1.615322492 1.069483222
  1.093465634 0.543326346 1.534507595 0.8956064037 1.102681364 1.247944095 1.343243045 0.8278437454
  1.074123497 1.10511465 1.163553173 1.35367029 1.444938273 1.247812218 2.044606827 1.66791974
  1.814245494 1.850727033 2.293174361 2.86757134 1.445655144 1.905589816 1.905836295 2.678941956
  0.9470991901 2.30357786 1.280820115 2.624874642 2.272150001 2.423425087 1.752337854 2.351488044
  1.42768323 1.784952475 4.583100289 4.590200748 4.235837595 2.235975547 4.479528048 3.905429093
  2.77173913 2.154917649 2.575948885 3.463731443 3.495540154 2.952260657 2.580921647 2.799350874
  0.8392128421 0.8981824681 1.290482315 1.513366156 1.630740345 1.958116358 2.070290944 1.704134411
  1.460644885 1.024991406 1.597635349 1.579748208 2.014861291 1.823278645 1.725221876 2.18211106
  1.63419949 9.12119899 2.604344482 2.560810478 7.272306817 2.144814101 4.22879912 3.971269651
  1.148378123 2.885561608 3.386790511 2.24410727 3.667585926 3.6281646 5.554563167 2.988292001
  0.73981318 1.328079804 0.8599845361 4.207969907 2.651997546 2.659803068 2.980492601 5.205279848
  1.659010432 2.10617949 1.380869113 1.575050722 1.672786117 1.920252235 1.841517137 2.532062015
  1.40130316 1.26268007 1.770432596 2.383527962 1.221285354 2.207371647 3.255160113 2.766930282
  1.424584258 1.30395912 1.174146268 1.212287734 1.060801108 1.929035982 1.907919832 1.716946703
  1.364809515 1.184819511 0.7477033611 1.773515327 1.681693586 1.17095626 1.964317964 3.107522573
  3.498139857 4.695283345 2.101462736 2.734989221 3.320163405 4.171190652 2.937827812 4.154683664
]))
(def voice_mode_gains (tensor @shape [232] @data [
  0.04791327778 0.07719747988 0.03704428278 0.09214963962 2.219587334e-18 7.663281408e-16 0.07129719854 0.01233385852
  7.676191016e-13 0.03892318125 7.580084305e-13 0.007112283369 7.638478289e-13 0.0129854403 0.01564977447 0.02829972005
  0.03681851136 0.03045952674 0.01378608046 0.01376293793 0.03608930089 1.053236466e-10 0.02067911135 0.01524023706
  0.004965159445 9.733288277e-10 0.01137518281 1.104434887e-26 0.006313056214 0.02201250373 5.059359902e-26 0.004909598024
  0.005294175264 0.01536111452 0.03822760668 0.01346625972 0.01627917574 0.009577891006 1.208386408e-10 0.004421043635
  0.0397083812 0.0005883493659 0.0001129528218 0.02023379971 0.008372982596 5.18236097e-09 0.01592982904 0.01869862827
  0.003531135735 3.846402002e-43 0.007829103514 0.005580845771 3.558404642e-30 0.00460425129 0.01930181821 0.0108818118
  0.007124388196 0.03636640745 0.01402805719 0.04192108846 2.459090932e-32 0.04122822019 0.01475628134 0.01263182154
  0.004304230982 0.004897689846 0.005928772221 0.005000171583 2.610340371e-32 1.217426052e-23 0.0107944084 0.008621994425
  0.051122358 0.01794442756 0.02183178587 0.02302105689 7.306853561e-13 0.03844176179 0.02335227726 0.04359645214
  0.01238479721 0.0001417474199 4.076504825e-12 0.009402556421 0.0350563708 0.0231076531 0.02477036861 0.02439111413
  0.02854273478 0.01278930295 0.00911178447 0.007168936517 0.01237700909 0.01341046424 0.01294246499 0.01706220641
  0.00649911025 0.006820105477 0.0009544501514 0.01204573617 0.01228077177 0.02513003501 0.01746083787 0.02299770178
  0.01751226912 0.01089601332 0.01889010389 7.210954326e-13 7.228598774e-13 0.02583831523 7.242792616e-13 0.01266598238
  7.155169052e-13 0.01384746517 7.163044411e-13 0.01290445741 0.02094013162 0.0161787335 0.01871069871 0.0164428098
  0.0189615194 0.01899986049 0.02690063932 0.04008511999 0.02714117337 0.02786002321 0.01281238445 0.01995093667
  0.00760334917 7.382544611e-13 0.0141502755 0.03198209684 0.03295098304 7.359803256e-13 0.01826488314 7.304358534e-13
  3.359219497e-20 2.962769013e-18 6.734668427e-12 0.04379609138 0.02084203871 0.007454850266 0.01666395615 0.009176778915
  0.03292845211 0.02677541228 2.737384045e-27 0.1078250531 0.03465583476 0.02336113397 0.004020369631 0.03067714104
  0.00808655719 1.426750929e-10 0.006911952435 0.01464806912 1.437245784e-10 1.441894024e-10 1.443141218e-10 0.009742457756
  0.01787856542 0.009092413833 0.01006629293 0.01337472553 0.003901253844 1.281273792e-12 3.982540989e-12 0.0002113286364
  0.01280502277 0.2415083825 7.277087214e-13 0.01973950467 0.1527127294 0.0307578183 0.06184033219 7.377146835e-13
  0.00402814599 0.01391220036 0.01445327824 7.593830586e-13 0.04176998332 0.01477634827 0.02924114405 0.02072477806
  0.03412561317 0.06581034267 0.03188847061 0.127908658 0.05665205459 0.03957423218 0.05105439689 0.1274993136
  0.04519044636 0.04490644066 0.03073681898 0.04466885728 1.629099808e-11 1.361862375e-11 0.03977107298 0.03795654259
  0.01918602951 0.03131347405 0.03163446108 0.07682620386 0.05300355752 0.04655627541 0.05149520694 0.03550562405
  0.02751965894 0.03053636504 0.03173654265 0.02074145968 0.02420305895 0.04120636658 0.04243992032 0.04633822304
  0.0329348485 0.03029826417 0.0007370092034 0.04202012806 0.03342258019 0.03295100688 0.04468099802 0.04924781354
  0.006657423801 0.02825578959 0.006943293613 0.01322402874 0.01901847754 0.02041585868 0.01146077477 0.03287114687
]))

(defmacro cymbal-smooth (target ms)
  (make-history previous)
  (make-history ready)
  (def pole (exp (/ -1 (* 0.001 ms samplerate))))
  (def value (gswitch (read-history ready) (mix target (read-history previous) pole) target))
  (write-history previous value)
  (write-history ready 1)
  value)
(defmacro cymbal-pole (input pole)
  (make-history previous)
  (def value (mix input (read-history previous) pole))
  (write-history previous value)
  value)
(make-history last_gate)
(def held (gt gate 0.5))
(def onset (max (gt trigger 0.5) (* held (lte (read-history last_gate) 0.5))))
(write-history last_gate held)
(def tick (max onset (eq (accum 1 0 0 16) 0)))
(def strength (latch (clip velocity 0 1) onset))
(def scale (latch (cymbal-smooth (/ (clip (mod size) 0.5 2)
  (pow (/ (clip pitch 65.406391 1046.50226) 261.625565) (clip tracking 0 1))) 12) tick))
(def decay_v (latch (cymbal-smooth (clip (mod decay) 0.2 3) 8) tick))
(def damping_v (latch (cymbal-smooth (clip (mod damping) 0.25 3) 8) tick))
(def touch_v (cymbal-smooth (clip (mod touch) 0 1) 2))
(def character_v (latch (cymbal-smooth (clip (mod character) 0 1) 12) tick))
(def voice_row (* character_v 28))
(def voice_lo (floor voice_row))
(def voice_hi (min 28 (+ voice_lo 1)))
(def voice_mix (- voice_row voice_lo))
(def voice_material_v (mix (gather voice_material (+ (iota 8) (* voice_lo 8))) (gather voice_material (+ (iota 8) (* voice_hi 8))) voice_mix))
(def voice_band_gains_v (mix (gather voice_band_gains (+ (iota 18) (* voice_lo 18))) (gather voice_band_gains (+ (iota 18) (* voice_hi 18))) voice_mix))
(def voice_mode_frequencies_v (mix (gather voice_mode_frequencies (+ (iota 8) (* voice_lo 8))) (gather voice_mode_frequencies (+ (iota 8) (* voice_hi 8))) voice_mix))
(def voice_mode_rates_v (mix (gather voice_mode_rates (+ (iota 8) (* voice_lo 8))) (gather voice_mode_rates (+ (iota 8) (* voice_hi 8))) voice_mix))
(def voice_mode_gains_v (mix (gather voice_mode_gains (+ (iota 8) (* voice_lo 8))) (gather voice_mode_gains (+ (iota 8) (* voice_hi 8))) voice_mix))
(def material voice_material_v)
(def band_gains voice_band_gains_v)
(def mode_frequencies voice_mode_frequencies_v)
(def mode_rates voice_mode_rates_v)
(def mode_gains voice_mode_gains_v)
(def base_rate0 (sample material 0))
(def base_rate1 (sample material 0.125))
(def base_rate2 (sample material 0.25))
(def base_rate3 (sample material 0.375))
(def base_rate4 (sample material 0.5))
(def base_rate5 (sample material 0.625))
(def base_contact_s (sample material 0.75))
(def base_direct (sample material 0.875))

;; Openness is a live loss condition, not an output envelope. Closed-hat
;; coefficients include contact losses; touch adds a choke to every wave path
;; and resolved resonance. Gate-off deliberately leaves percussion ringing.
(def contact_loss (* 180 touch_v touch_v))

;; Finite stick contact: resolved modes receive the compression impulse;
;; unresolved modes receive the compact, stochastic rough-contact force.
;; Randomness exists only while the stick contacts the metal; all subsequent
;; sound is the free response of the passive body.
(def hardness_v (latch (clip (mod hardness) 0 1) tick))
(def contact_s (latch (* base_contact_s (pow 2 (* 2 (- 0.5 hardness_v)))) tick))
(def age (accum (/ 1 samplerate) onset 0 100000))
(def pulse_phase (clip (/ age contact_s) 0 1))
(def friction (* (noise) (sin (* pi pulse_phase)) (lt age contact_s)))
(def force_pole (exp (/ -1 (* samplerate 0.000025 (pow 2 (* 3 (- 0.5 hardness_v)))))))
(def force (cymbal-pole (cymbal-pole
  (* strength friction 0.23) force_pole) force_pole))
(def point_force (cymbal-pole (cymbal-pole (* onset strength) force_pole) force_pole))
(def region_rate0 (+ (/ (* base_rate0 (pow damping_v 0)) decay_v) contact_loss))
;; Region 0, path 0: lossless fractional propagation.
(make-history outgoing0)
(def path_samples0 (max 8 (* 109 scale (/ samplerate 48000))))
(def integer_delay0 (- (floor path_samples0) 2))
(def fractional0 (+ 1 (- path_samples0 (floor path_samples0))))
(def allpass_a0 (/ (- 1 fractional0) (+ 1 fractional0)))
(def incoming0 (delay (read-history outgoing0) integer_delay0))
(make-history ap_x0)
(make-history ap_y0)
(def arrival0 (- (+ (* allpass_a0 incoming0) (read-history ap_x0)) (* allpass_a0 (read-history ap_y0))))
(write-history ap_x0 incoming0)
(write-history ap_y0 arrival0)
(def path_seconds0 (/ path_samples0 samplerate))
(def damped0 (* (exp (* (- region_rate0) path_seconds0)) arrival0))
;; Region 0, path 1: lossless fractional propagation.
(make-history outgoing1)
(def path_samples1 (max 8 (* 461 scale (/ samplerate 48000))))
(def integer_delay1 (- (floor path_samples1) 2))
(def fractional1 (+ 1 (- path_samples1 (floor path_samples1))))
(def allpass_a1 (/ (- 1 fractional1) (+ 1 fractional1)))
(def incoming1 (delay (read-history outgoing1) integer_delay1))
(make-history ap_x1)
(make-history ap_y1)
(def arrival1 (- (+ (* allpass_a1 incoming1) (read-history ap_x1)) (* allpass_a1 (read-history ap_y1))))
(write-history ap_x1 incoming1)
(write-history ap_y1 arrival1)
(def path_seconds1 (/ path_samples1 samplerate))
(def damped1 (* (exp (* (- region_rate0) path_seconds1)) arrival1))
;; Region 0, path 2: lossless fractional propagation.
(make-history outgoing2)
(def path_samples2 (max 8 (* 1797 scale (/ samplerate 48000))))
(def integer_delay2 (- (floor path_samples2) 2))
(def fractional2 (+ 1 (- path_samples2 (floor path_samples2))))
(def allpass_a2 (/ (- 1 fractional2) (+ 1 fractional2)))
(def incoming2 (delay (read-history outgoing2) integer_delay2))
(make-history ap_x2)
(make-history ap_y2)
(def arrival2 (- (+ (* allpass_a2 incoming2) (read-history ap_x2)) (* allpass_a2 (read-history ap_y2))))
(write-history ap_x2 incoming2)
(write-history ap_y2 arrival2)
(def path_seconds2 (/ path_samples2 samplerate))
(def damped2 (* (exp (* (- region_rate0) path_seconds2)) arrival2))
(def region_rate1 (+ (/ (* base_rate1 (pow damping_v 0.2)) decay_v) contact_loss))
;; Region 1, path 0: lossless fractional propagation.
(make-history outgoing3)
(def path_samples3 (max 8 (* 130 scale (/ samplerate 48000))))
(def integer_delay3 (- (floor path_samples3) 2))
(def fractional3 (+ 1 (- path_samples3 (floor path_samples3))))
(def allpass_a3 (/ (- 1 fractional3) (+ 1 fractional3)))
(def incoming3 (delay (read-history outgoing3) integer_delay3))
(make-history ap_x3)
(make-history ap_y3)
(def arrival3 (- (+ (* allpass_a3 incoming3) (read-history ap_x3)) (* allpass_a3 (read-history ap_y3))))
(write-history ap_x3 incoming3)
(write-history ap_y3 arrival3)
(def path_seconds3 (/ path_samples3 samplerate))
(def damped3 (* (exp (* (- region_rate1) path_seconds3)) arrival3))
;; Region 1, path 1: lossless fractional propagation.
(make-history outgoing4)
(def path_samples4 (max 8 (* 549 scale (/ samplerate 48000))))
(def integer_delay4 (- (floor path_samples4) 2))
(def fractional4 (+ 1 (- path_samples4 (floor path_samples4))))
(def allpass_a4 (/ (- 1 fractional4) (+ 1 fractional4)))
(def incoming4 (delay (read-history outgoing4) integer_delay4))
(make-history ap_x4)
(make-history ap_y4)
(def arrival4 (- (+ (* allpass_a4 incoming4) (read-history ap_x4)) (* allpass_a4 (read-history ap_y4))))
(write-history ap_x4 incoming4)
(write-history ap_y4 arrival4)
(def path_seconds4 (/ path_samples4 samplerate))
(def damped4 (* (exp (* (- region_rate1) path_seconds4)) arrival4))
;; Region 1, path 2: lossless fractional propagation.
(make-history outgoing5)
(def path_samples5 (max 8 (* 2141 scale (/ samplerate 48000))))
(def integer_delay5 (- (floor path_samples5) 2))
(def fractional5 (+ 1 (- path_samples5 (floor path_samples5))))
(def allpass_a5 (/ (- 1 fractional5) (+ 1 fractional5)))
(def incoming5 (delay (read-history outgoing5) integer_delay5))
(make-history ap_x5)
(make-history ap_y5)
(def arrival5 (- (+ (* allpass_a5 incoming5) (read-history ap_x5)) (* allpass_a5 (read-history ap_y5))))
(write-history ap_x5 incoming5)
(write-history ap_y5 arrival5)
(def path_seconds5 (/ path_samples5 samplerate))
(def damped5 (* (exp (* (- region_rate1) path_seconds5)) arrival5))
(def region_rate2 (+ (/ (* base_rate2 (pow damping_v 0.4)) decay_v) contact_loss))
;; Region 2, path 0: lossless fractional propagation.
(make-history outgoing6)
(def path_samples6 (max 8 (* 153 scale (/ samplerate 48000))))
(def integer_delay6 (- (floor path_samples6) 2))
(def fractional6 (+ 1 (- path_samples6 (floor path_samples6))))
(def allpass_a6 (/ (- 1 fractional6) (+ 1 fractional6)))
(def incoming6 (delay (read-history outgoing6) integer_delay6))
(make-history ap_x6)
(make-history ap_y6)
(def arrival6 (- (+ (* allpass_a6 incoming6) (read-history ap_x6)) (* allpass_a6 (read-history ap_y6))))
(write-history ap_x6 incoming6)
(write-history ap_y6 arrival6)
(def path_seconds6 (/ path_samples6 samplerate))
(def damped6 (* (exp (* (- region_rate2) path_seconds6)) arrival6))
;; Region 2, path 1: lossless fractional propagation.
(make-history outgoing7)
(def path_samples7 (max 8 (* 650 scale (/ samplerate 48000))))
(def integer_delay7 (- (floor path_samples7) 2))
(def fractional7 (+ 1 (- path_samples7 (floor path_samples7))))
(def allpass_a7 (/ (- 1 fractional7) (+ 1 fractional7)))
(def incoming7 (delay (read-history outgoing7) integer_delay7))
(make-history ap_x7)
(make-history ap_y7)
(def arrival7 (- (+ (* allpass_a7 incoming7) (read-history ap_x7)) (* allpass_a7 (read-history ap_y7))))
(write-history ap_x7 incoming7)
(write-history ap_y7 arrival7)
(def path_seconds7 (/ path_samples7 samplerate))
(def damped7 (* (exp (* (- region_rate2) path_seconds7)) arrival7))
;; Region 2, path 2: lossless fractional propagation.
(make-history outgoing8)
(def path_samples8 (max 8 (* 2535 scale (/ samplerate 48000))))
(def integer_delay8 (- (floor path_samples8) 2))
(def fractional8 (+ 1 (- path_samples8 (floor path_samples8))))
(def allpass_a8 (/ (- 1 fractional8) (+ 1 fractional8)))
(def incoming8 (delay (read-history outgoing8) integer_delay8))
(make-history ap_x8)
(make-history ap_y8)
(def arrival8 (- (+ (* allpass_a8 incoming8) (read-history ap_x8)) (* allpass_a8 (read-history ap_y8))))
(write-history ap_x8 incoming8)
(write-history ap_y8 arrival8)
(def path_seconds8 (/ path_samples8 samplerate))
(def damped8 (* (exp (* (- region_rate2) path_seconds8)) arrival8))
(def region_rate3 (+ (/ (* base_rate3 (pow damping_v 0.6)) decay_v) contact_loss))
;; Region 3, path 0: lossless fractional propagation.
(make-history outgoing9)
(def path_samples9 (max 8 (* 177 scale (/ samplerate 48000))))
(def integer_delay9 (- (floor path_samples9) 2))
(def fractional9 (+ 1 (- path_samples9 (floor path_samples9))))
(def allpass_a9 (/ (- 1 fractional9) (+ 1 fractional9)))
(def incoming9 (delay (read-history outgoing9) integer_delay9))
(make-history ap_x9)
(make-history ap_y9)
(def arrival9 (- (+ (* allpass_a9 incoming9) (read-history ap_x9)) (* allpass_a9 (read-history ap_y9))))
(write-history ap_x9 incoming9)
(write-history ap_y9 arrival9)
(def path_seconds9 (/ path_samples9 samplerate))
(def damped9 (* (exp (* (- region_rate3) path_seconds9)) arrival9))
;; Region 3, path 1: lossless fractional propagation.
(make-history outgoing10)
(def path_samples10 (max 8 (* 751 scale (/ samplerate 48000))))
(def integer_delay10 (- (floor path_samples10) 2))
(def fractional10 (+ 1 (- path_samples10 (floor path_samples10))))
(def allpass_a10 (/ (- 1 fractional10) (+ 1 fractional10)))
(def incoming10 (delay (read-history outgoing10) integer_delay10))
(make-history ap_x10)
(make-history ap_y10)
(def arrival10 (- (+ (* allpass_a10 incoming10) (read-history ap_x10)) (* allpass_a10 (read-history ap_y10))))
(write-history ap_x10 incoming10)
(write-history ap_y10 arrival10)
(def path_seconds10 (/ path_samples10 samplerate))
(def damped10 (* (exp (* (- region_rate3) path_seconds10)) arrival10))
;; Region 3, path 2: lossless fractional propagation.
(make-history outgoing11)
(def path_samples11 (max 8 (* 2929 scale (/ samplerate 48000))))
(def integer_delay11 (- (floor path_samples11) 2))
(def fractional11 (+ 1 (- path_samples11 (floor path_samples11))))
(def allpass_a11 (/ (- 1 fractional11) (+ 1 fractional11)))
(def incoming11 (delay (read-history outgoing11) integer_delay11))
(make-history ap_x11)
(make-history ap_y11)
(def arrival11 (- (+ (* allpass_a11 incoming11) (read-history ap_x11)) (* allpass_a11 (read-history ap_y11))))
(write-history ap_x11 incoming11)
(write-history ap_y11 arrival11)
(def path_seconds11 (/ path_samples11 samplerate))
(def damped11 (* (exp (* (- region_rate3) path_seconds11)) arrival11))
(def region_rate4 (+ (/ (* base_rate4 (pow damping_v 0.8)) decay_v) contact_loss))
;; Region 4, path 0: lossless fractional propagation.
(make-history outgoing12)
(def path_samples12 (max 8 (* 204 scale (/ samplerate 48000))))
(def integer_delay12 (- (floor path_samples12) 2))
(def fractional12 (+ 1 (- path_samples12 (floor path_samples12))))
(def allpass_a12 (/ (- 1 fractional12) (+ 1 fractional12)))
(def incoming12 (delay (read-history outgoing12) integer_delay12))
(make-history ap_x12)
(make-history ap_y12)
(def arrival12 (- (+ (* allpass_a12 incoming12) (read-history ap_x12)) (* allpass_a12 (read-history ap_y12))))
(write-history ap_x12 incoming12)
(write-history ap_y12 arrival12)
(def path_seconds12 (/ path_samples12 samplerate))
(def damped12 (* (exp (* (- region_rate4) path_seconds12)) arrival12))
;; Region 4, path 1: lossless fractional propagation.
(make-history outgoing13)
(def path_samples13 (max 8 (* 864 scale (/ samplerate 48000))))
(def integer_delay13 (- (floor path_samples13) 2))
(def fractional13 (+ 1 (- path_samples13 (floor path_samples13))))
(def allpass_a13 (/ (- 1 fractional13) (+ 1 fractional13)))
(def incoming13 (delay (read-history outgoing13) integer_delay13))
(make-history ap_x13)
(make-history ap_y13)
(def arrival13 (- (+ (* allpass_a13 incoming13) (read-history ap_x13)) (* allpass_a13 (read-history ap_y13))))
(write-history ap_x13 incoming13)
(write-history ap_y13 arrival13)
(def path_seconds13 (/ path_samples13 samplerate))
(def damped13 (* (exp (* (- region_rate4) path_seconds13)) arrival13))
;; Region 4, path 2: lossless fractional propagation.
(make-history outgoing14)
(def path_samples14 (max 8 (* 3372 scale (/ samplerate 48000))))
(def integer_delay14 (- (floor path_samples14) 2))
(def fractional14 (+ 1 (- path_samples14 (floor path_samples14))))
(def allpass_a14 (/ (- 1 fractional14) (+ 1 fractional14)))
(def incoming14 (delay (read-history outgoing14) integer_delay14))
(make-history ap_x14)
(make-history ap_y14)
(def arrival14 (- (+ (* allpass_a14 incoming14) (read-history ap_x14)) (* allpass_a14 (read-history ap_y14))))
(write-history ap_x14 incoming14)
(write-history ap_y14 arrival14)
(def path_seconds14 (/ path_samples14 samplerate))
(def damped14 (* (exp (* (- region_rate4) path_seconds14)) arrival14))
(def region_rate5 (+ (/ (* base_rate5 (pow damping_v 1)) decay_v) contact_loss))
;; Region 5, path 0: lossless fractional propagation.
(make-history outgoing15)
(def path_samples15 (max 8 (* 234 scale (/ samplerate 48000))))
(def integer_delay15 (- (floor path_samples15) 2))
(def fractional15 (+ 1 (- path_samples15 (floor path_samples15))))
(def allpass_a15 (/ (- 1 fractional15) (+ 1 fractional15)))
(def incoming15 (delay (read-history outgoing15) integer_delay15))
(make-history ap_x15)
(make-history ap_y15)
(def arrival15 (- (+ (* allpass_a15 incoming15) (read-history ap_x15)) (* allpass_a15 (read-history ap_y15))))
(write-history ap_x15 incoming15)
(write-history ap_y15 arrival15)
(def path_seconds15 (/ path_samples15 samplerate))
(def damped15 (* (exp (* (- region_rate5) path_seconds15)) arrival15))
;; Region 5, path 1: lossless fractional propagation.
(make-history outgoing16)
(def path_samples16 (max 8 (* 991 scale (/ samplerate 48000))))
(def integer_delay16 (- (floor path_samples16) 2))
(def fractional16 (+ 1 (- path_samples16 (floor path_samples16))))
(def allpass_a16 (/ (- 1 fractional16) (+ 1 fractional16)))
(def incoming16 (delay (read-history outgoing16) integer_delay16))
(make-history ap_x16)
(make-history ap_y16)
(def arrival16 (- (+ (* allpass_a16 incoming16) (read-history ap_x16)) (* allpass_a16 (read-history ap_y16))))
(write-history ap_x16 incoming16)
(write-history ap_y16 arrival16)
(def path_seconds16 (/ path_samples16 samplerate))
(def damped16 (* (exp (* (- region_rate5) path_seconds16)) arrival16))
;; Region 5, path 2: lossless fractional propagation.
(make-history outgoing17)
(def path_samples17 (max 8 (* 3864 scale (/ samplerate 48000))))
(def integer_delay17 (- (floor path_samples17) 2))
(def fractional17 (+ 1 (- path_samples17 (floor path_samples17))))
(def allpass_a17 (/ (- 1 fractional17) (+ 1 fractional17)))
(def incoming17 (delay (read-history outgoing17) integer_delay17))
(make-history ap_x17)
(make-history ap_y17)
(def arrival17 (- (+ (* allpass_a17 incoming17) (read-history ap_x17)) (* allpass_a17 (read-history ap_y17))))
(write-history ap_x17 incoming17)
(write-history ap_y17 arrival17)
(def path_seconds17 (/ path_samples17 samplerate))
(def damped17 (* (exp (* (- region_rate5) path_seconds17)) arrival17))
(def scattered0 damped0)
(def scattered1 damped1)
(def scattered2 damped2)
(def junction0 (* 0.666666666666667 (+ scattered0 scattered1 scattered2)))
(write-history outgoing0 (+ (- junction0 scattered0) (* force 0.57735026919)))
(write-history outgoing1 (+ (- junction0 scattered1) (* force -0.57735026919)))
(write-history outgoing2 (+ (- junction0 scattered2) (* force 0.57735026919)))
(def plate0 (+ (* force base_direct) (* scattered0 0.485823499594) (* scattered1 -0.147516199622) (* scattered2 -0.268275790313)))
(def scattered3 damped3)
(def scattered4 damped4)
(def scattered5 damped5)
(def junction1 (* 0.666666666666667 (+ scattered3 scattered4 scattered5)))
(write-history outgoing3 (+ (- junction1 scattered3) (* force 0.57735026919)))
(write-history outgoing4 (+ (- junction1 scattered4) (* force -0.57735026919)))
(write-history outgoing5 (+ (- junction1 scattered5) (* force 0.57735026919)))
(def plate1 (+ (* force base_direct) (* scattered3 0.485823499594) (* scattered4 -0.147516199622) (* scattered5 -0.268275790313)))
(def scattered6 damped6)
(def scattered7 damped7)
(def scattered8 damped8)
(def junction2 (* 0.666666666666667 (+ scattered6 scattered7 scattered8)))
(write-history outgoing6 (+ (- junction2 scattered6) (* force 0.57735026919)))
(write-history outgoing7 (+ (- junction2 scattered7) (* force -0.57735026919)))
(write-history outgoing8 (+ (- junction2 scattered8) (* force 0.57735026919)))
(def plate2 (+ (* force base_direct) (* scattered6 0.485823499594) (* scattered7 -0.147516199622) (* scattered8 -0.268275790313)))
(def scattered9 damped9)
(def scattered10 damped10)
(def scattered11 damped11)
(def junction3 (* 0.666666666666667 (+ scattered9 scattered10 scattered11)))
(write-history outgoing9 (+ (- junction3 scattered9) (* force 0.57735026919)))
(write-history outgoing10 (+ (- junction3 scattered10) (* force -0.57735026919)))
(write-history outgoing11 (+ (- junction3 scattered11) (* force 0.57735026919)))
(def plate3 (+ (* force base_direct) (* scattered9 0.485823499594) (* scattered10 -0.147516199622) (* scattered11 -0.268275790313)))
(def scattered12 damped12)
(def scattered13 damped13)
(def scattered14 damped14)
(def junction4 (* 0.666666666666667 (+ scattered12 scattered13 scattered14)))
(write-history outgoing12 (+ (- junction4 scattered12) (* force 0.57735026919)))
(write-history outgoing13 (+ (- junction4 scattered13) (* force -0.57735026919)))
(write-history outgoing14 (+ (- junction4 scattered14) (* force 0.57735026919)))
(def plate4 (+ (* force base_direct) (* scattered12 0.485823499594) (* scattered13 -0.147516199622) (* scattered14 -0.268275790313)))
(def scattered15 damped15)
(def scattered16 damped16)
(def scattered17 damped17)
(def junction5 (* 0.666666666666667 (+ scattered15 scattered16 scattered17)))
(write-history outgoing15 (+ (- junction5 scattered15) (* force 0.57735026919)))
(write-history outgoing16 (+ (- junction5 scattered16) (* force -0.57735026919)))
(write-history outgoing17 (+ (- junction5 scattered17) (* force 0.57735026919)))
(def plate5 (+ (* force base_direct) (* scattered15 0.485823499594) (* scattered16 -0.147516199622) (* scattered17 -0.268275790313)))
(def radiation_hz (tensor @shape [18] @data [100 135.944938584 184.810263266 251.240198894 341.548334086 464.317673008 631.216375405 858.106713878 1166.55264517 1585.86927702 2155.90901467 2930.84918593 3984.3411258 5416.51009645 7363.47132402 10010.2665691 13608.4507395 18500]))
(def region_ids (tensor @shape [18] @data [0 0 0 1 1 1 2 2 2 3 3 3 4 4 4 5 5 5]))
(def plate_vector (+ (* plate0 (eq region_ids 0)) (* plate1 (eq region_ids 1)) (* plate2 (eq region_ids 2)) (* plate3 (eq region_ids 3)) (* plate4 (eq region_ids 4)) (* plate5 (eq region_ids 5))))
(def filter_hz (min (/ radiation_hz scale) (* samplerate 0.43)))
(def filter_g (tan (* pi (/ filter_hz samplerate))))
(def filter_a1 (/ 1 (+ 1 (* filter_g (+ filter_g 0.3125)))))
(def filter_a2 (* filter_g filter_a1))
(def filter_a3 (* filter_g filter_a2))
(make-tensor-history filter_ic1 @shape [18])
(make-tensor-history filter_ic2 @shape [18])
(def ic1 (read-tensor-history filter_ic1))
(def ic2 (read-tensor-history filter_ic2))
(def v3 (- plate_vector ic2))
(def v1 (+ (* filter_a1 ic1) (* filter_a2 v3)))
(def v2 (+ ic2 (* filter_a2 ic1) (* filter_a3 v3)))
(write-tensor-history filter_ic1 (- (* 2 v1) ic1))
(write-tensor-history filter_ic2 (- (* 2 v2) ic2))
(def radiation_bands v1)
(def frequencies (/ mode_frequencies scale))
(def omega (* twopi (/ (min frequencies (* samplerate 0.47)) samplerate)))
(def radius (exp (/ (- (+ (/ mode_rates decay_v) contact_loss)) samplerate)))
(def c (* radius (cos omega)))
(def s (* radius (sin omega)))
(def band (clip (/ (- (* samplerate 0.47) frequencies) (* samplerate 0.06)) 0 1))
;; Deconvolve the reference two-pole contact response, without recorded phase.
(def reference_pole (exp (/ -1 (* samplerate 0.000025))))
(def compensation (/ (+ (* (- 1 reference_pole) (- 1 reference_pole))
  (* 2 reference_pole (- 1 (cos omega)))) (* (- 1 reference_pole) (- 1 reference_pole))))
(make-tensor-history mode_r @shape [8])
(make-tensor-history mode_i @shape [8])
(def x (read-tensor-history mode_r))
(def y (read-tensor-history mode_i))
(def next_r (+ (- (* c x) (* s y)) (* point_force band compensation)))
(def next_i (+ (* s x) (* c y)))
(write-tensor-history mode_r next_r)
(write-tensor-history mode_i next_i)
(def resolved next_i)
(def color_v (latch (cymbal-smooth (clip (mod color) -1 1) 8) tick))
(def wash_v (cymbal-smooth (clip (mod wash) 0 2) 8))
(def bell_v (cymbal-smooth (clip (mod bell) 0 2) 8))
(def width_v (cymbal-smooth (clip (mod width) 0 1) 8))
(def gain_v (cymbal-smooth (clip (mod gain) 0 2) 8))
(def radiation (* radiation_bands band_gains (pow (/ radiation_hz 2800) (* color_v 0.6))))
(def band_pan (tensor @shape [18] @data [0 0.303970632 -0.448276968 0.357120338 -0.0783818782 -0.241527623 0.434571783 -0.399351794 0.154367385 0.171700383 -0.407580422 0.429373855 -0.225633413 -0.0966237415 0.368128093 -0.446268656 0.290001144 0.0185930197]))
(def body_left (sum (* radiation (+ 1 (* width_v band_pan)))))
(def body_right (sum (* radiation (- 1 (* width_v band_pan)))))
(def modal_pan (tensor @shape [8] @data [0 0.4 -0.5 0.3 -0.2 0.5 -0.4 0.1]))
(def mode_signal (* resolved mode_gains))
(def bell_left (sum (* mode_signal (+ 1 (* width_v modal_pan)))))
(def bell_right (sum (* mode_signal (- 1 (* width_v modal_pan)))))
(out (* 0.65 gain_v (+ (* wash_v body_left) (* bell_v bell_left))) 1 @name left)
(out (* 0.65 gain_v (+ (* wash_v body_right) (* bell_v bell_right))) 2 @name right)
