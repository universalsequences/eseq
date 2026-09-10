;; PM Crash / reduced dispersive plate, identified from Donit's cymbal recordings.
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
(param bell @group body @default 0.5 @min 0 @max 2 @mod true @mod-mode additive)
(param wash @group body @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param touch @group contact @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param color @group output @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param width @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param gain @group output @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param tracking @group tuning @default 0 @min 0 @max 1)
;; Calibration SHA256: e1e3d4cc671d29fc08e345863b8f20b8cb6dca1a8d32bc0a6d689a3982ff4e5d
(def voice_material (tensor @shape [184] @data [
  0.1022406704 3.619088058 1.464566881 2.013055371 4.570933354 5.660812618 0.0006 0.3
  0.1000000001 2.964057653 0.8506487996 0.8476070701 0.9796980104 6.99479444 0.0006 0
  0.1000000001 7.746821047 1.956348318 2.70772592 4.353687676 13.84994556 0.0006 0
  0.231523907 1.323942731 3.170475648 2.857986059 3.30143554 17.59104298 0.0006 0
  9.087142342 0.9981994092 0.2452320613 10.24949285 1.973943939 11.55397017 0.0006 1
  0.1540785436 0.6300917579 0.5152194247 0.8582243665 3.579784045 8.854014703 0.0006 0
  0.1000000803 1.026711235 1.069962914 0.8884441266 1.298711019 5.496832104 0.0006 0
  0.3329017913 2.161788271 0.2993830479 0.8639095878 2.808823478 6.689379067 0.008 0
  0.1035127098 8.344443923 0.3639467201 0.5837297907 1.88317274 10.21851236 0.0006 0
  0.2503232127 0.7764269357 0.89234251 1.155977986 3.277703863 5.415902523 0.008 0.3
  0.1046150685 0.5326324679 1.087223446 1.185662675 2.346064569 7.58137254 0.0025 1
  0.5025441628 1.623714161 3.724374868 2.112982775 3.114681988 15.60349165 0.0006 0
  0.1914717002 0.7399857417 1.172544192 1.968346761 2.066733771 6.893524085 0.0006 0
  1.712959072 0.8725237168 1.23044646 1.146286993 4.509351655 10.74393793 0.0006 0.3
  0.5069297173 1.742756314 0.6229063 0.6027402583 2.730439377 8.670717308 0.0006 0
  0.1000115293 0.838835119 1.677309914 1.20400003 3.68215285 8.458427416 0.008 0.3
  0.8552382751 5.872734827 1.170629085 1.127254603 2.540722258 7.004742045 0.0006 1
  0.2110624291 1.986512364 2.584296866 2.772426438 2.69159163 10.73529844 0.0006 0
  0.1333072138 0.5172345976 2.301476369 1.196978618 1.984227821 6.961316505 0.008 0
  0.1 0.1 0.6096541303 0.9733648705 1.912776431 4.542441939 0.0006 0
  0.1010248654 0.4665400668 4.622839856 1.008989741 1.332212237 5.076984513 0.0006 0
  1.058946807 9.628724938 2.700868319 5.805468849 6.92805761 6.358153656 0.0006 0.3
  0.7276187443 8.07149048 3.518642872 5.223663467 5.673430082 7.175 0.0006 0
]))
(def voice_band_gains (tensor @shape [414] @data [
  0.6865479229 8.733752284e-11 8.220609747e-11 7.457118412e-11 0.2056434004 5.885145853e-11 5.558803792e-11 1.302261854 0.155411045 8.173131491e-11 1.210406556e-10 6.533744995 1.75873448e-10 2.196226425e-10 11.04232896 4.412393505e-10 7.963643422 10.86913645
  0.3163436929 1.18467389e-12 0.5768521493 1.298346869e-12 4.252439177 8.518628762e-13 9.074649802e-13 2.259363531 8.912337182e-13 1.281741355e-12 14.31130876 2.423408352e-12 2.785989682e-12 24.86828843 24.83637923 83.83092598 25.81579018 9.004401937
  1.074958814 9.894490838e-11 7.613969074e-11 9.137524502e-11 0.244379135 7.713731469e-11 0.1877452873 7.02101929e-11 5.048317555 3.755133517 1.577722831e-10 19.60552372 2.441341772e-10 3.045548037e-10 17.24963952 28.24678684 16.18416708 18.20178326
  1.676783577 1.056407924e-10 0.1525852044 8.718141054e-11 1.119940322e-10 0.9608643263 7.993638599e-11 11.26212929 7.811444923e-11 1.134321471e-10 1.896391759 21.417621 2.481863923e-10 22.09368735 21.71749015 46.99163394 32.5054622 18.07521447
  5.433785082e-11 4.611768371e-11 4.793943961e-11 3.534481608e-11 4.901766708 2.660164018 2.523565776e-11 2.548307746e-11 2.298942198 3.453232832e-11 5.249671784e-11 7.055425478e-11 17.09110178 8.790966626e-11 25.50441367 1.739455475e-10 19.07175869 3.073455203e-10
  1.31219415 2.738081293e-19 0.5195245056 4.252162862e-20 0.831956675 1.781697022 7.00775784e-19 7.624657179e-11 1.466823574 2.78124766e-18 4.457582681e-07 12.09446512 1.871638736e-17 7.190496311 42.84558639 60.6429667 59.35840092 43.21166474
  0.5033287635 0.03853386143 5.887642655e-10 8.283092705e-09 6.310883404e-06 2.891568692 2.00671067 0.04538986599 2.945828898 8.17223727e-09 1.221937212e-08 21.85991852 2.684023571e-08 17.01051016 38.3004635 50.1909538 46.49635148 46.56557045
  0.2974820405 4.093979412e-11 0.1650678494 0.1332025104 1.163976868e-10 1.03958785e-10 1.404674317e-10 0.5837212287 1.240041524e-10 1.73883841 1.757924917 0.5385777818 8.770553401e-11 4.499667621 1.090405639e-10 4.590578323 6.450872041 1.300040735
  1.949169153 8.268823315e-20 0.8300157458 1.663514718 0.1995697404 0.3352366442 4.392016842e-18 1.045700306e-12 4.495999175 5.653238095e-16 4.288232661e-13 9.896737455 1.651177233e-13 51.24997648 3.4709246e-09 53.3159447 9.703598716 1.286851156e-12
  0.2189665488 3.618784337e-36 0.2041136422 7.091912015e-38 1.882135087e-33 2.387086071 3.533652727e-33 4.810843433 1.093952312e-32 1.197572798 8.112824454 2.898038431 0.003460437747 1.917889353 10.53522076 5.00306835e-35 11.30423076 8.519974334
  0.4870406895 1.164324343e-11 0.1979476585 1.278880453e-11 0.4685835062 0.2479937926 3.684558723e-11 1.044310177 4.23190028e-11 3.810135762 0.1577329714 4.847291632e-11 21.2258682 6.33612546e-11 8.590895077 9.261447067e-11 5.340733 4.723990351
  1.802396622 3.866823676e-09 0.1013191773 7.073867095e-09 1.951141268e-08 2.156871985 1.065446625 1.355548668e-08 0.1441828324 5.961977458 3.429492797 24.96008144 3.835750029e-08 5.459879498e-08 52.94896107 1.255467817e-07 62.35006493 35.55767377
  1.586560427 1.441969861e-10 0.369354261 1.965697339e-09 4.24450124e-09 0.0372953617 1.602461288e-09 2.349801353 1.138962181 1.255125379 5.467027184e-09 7.462352728e-09 17.99197235 11.41613611 35.13397917 56.91227706 41.58466599 39.84867595
  0.6535692243 9.420628896e-11 1.349987639 7.886138335e-11 2.871843074 6.129020323e-11 1.385953663 0.1866781383 1.494602643 8.947781113e-11 1.42146438e-10 26.90361483 1.93863349e-10 8.26816121 51.41736332 4.777472082e-10 43.707696 61.89722728
  1.820597323 1.842484178e-08 0.4275370755 1.703866241e-08 1.973964364e-08 1.767752933 1.502178365e-08 0.7913596874 1.477641374e-08 0.3047364486 1.902737917 8.494619388 4.813048765e-08 5.029153422 50.11247461 61.40500533 98.94625709 83.50157789
  0.1011843558 6.555740599e-12 1.049945172e-11 1.415219896e-11 2.201782256e-11 2.096729704 2.292912401e-11 1.883204101 2.203255727e-11 2.674713575 2.148315837 1.44742967 5.090101706 4.670061419 5.150105343 11.68778492 19.45666384 12.18012206
  0.1745761845 0.4901696691 0.4487109241 2.186171177 2.912642451e-11 2.61573615e-11 1.984894487 1.913553474e-11 1.731935048e-11 2.874539936e-11 7.108091508 6.022376307 3.788016632e-11 7.223199241e-11 18.40607538 5.82370686e-11 24.98210333 29.11014805
  1.410667779 1.015862055e-10 0.2221404923 8.853848937e-11 1.112392535e-10 3.399897755 0.7175711671 8.430642864 1.693946995 1.1185472e-10 1.893214044e-10 24.09595636 2.511115529e-10 26.70685811 20.01678687 45.14309051 9.661195718 1.177065812e-09
  0.1121111113 3.049994424e-10 0.5203102883 9.418867616e-11 0.3971324599 1.600282002e-10 5.345777483e-10 0.7051479503 5.527779324e-10 3.534724952e-10 5.513268619 1.00201232e-09 10.32379521 2.92165944 1.716905403 9.36313533 11.54627607 0.6129285921
  6.951782186e-10 6.091645322e-10 0.07878223145 4.386044983e-10 4.839118051e-09 0.9809125253 8.264391605e-09 1.120035108e-08 0.8353908737 0.09897437093 2.893067045e-09 9.227557506 5.646244323e-09 11.84668502 16.72749056 111.8414136 11.46767604 21.38455321
  0.3106179669 1.938792868e-08 1.511779045e-08 1.582297966e-08 1.94219097e-08 1.350021213 0.6396572524 0.06432957555 1.531917718e-08 2.110620772e-08 3.038215967e-08 5.270244401 4.618553033e-08 5.822020441e-08 49.92664 1.242872838e-07 136.0724886 2.191236113e-07
  1.862737643 8.229487517e-11 0.4774667087 7.252552936e-11 2.294114684 5.706738671e-11 0.9294722575 2.374902119 5.713514664e-11 7.771131381e-11 1.153947948e-10 41.57430666 1.656495331e-10 2.067836885e-10 2.973743465e-10 60.12155864 69.63732163 81.28683223
  2.656117084 1.010893594e-10 0.0169885094 9.038688622e-11 1.042461001e-10 7.522687036e-11 1.202330131 3.145136267 7.746663806e-11 1.125103187e-10 10.90461512 2.143593901e-10 2.497766237e-10 30.68834263 75.51997045 6.432045267e-10 89.03986505 150.9633073
]))
(def voice_mode_frequencies (tensor @shape [184] @data [
  714.3818099 814.0744942 836.5715184 914.8417918 946.2165578 981.8833257 1017.231242 1067.577499
  372.9248133 469.179751 483.8929786 713.7173148 1514.429008 2716.566015 4136.945441 4543.036971
  490.4425085 805.731922 902.2166238 954.5925155 1959.25453 2947.176146 3027.28657 4099.452316
  450.6426357 728.4326961 877.3448381 956.3731462 1965.70991 2892.111511 3093.158219 3129.010961
  377.5935975 448.368512 464.1046049 502.1275828 567.1106014 3350.42407 3521.231986 3628.312099
  334.8044377 961.7075306 1696.84369 1782.692454 2204.289256 2294.616887 2367.574956 2843.87788
  303.5593069 421.2041977 513.4347382 575.445861 1092.815881 1533.25699 2941.8518 4619.806958
  349.1127578 441.4235569 534.2254286 609.250656 642.3807455 661.3899377 3670.616821 3733.808949
  1011.599949 1450.342287 1549.729439 1797.90991 1971.919359 2114.389017 2324.894502 3233.228715
  318.3047284 352.6547556 443.2834441 657.2945801 779.2421986 1745.717319 2332.527474 2392.467241
  1258.272191 1380.614949 1475.401377 1745.556665 1771.844967 1820.651994 1926.815705 2350.104698
  715.6671364 759.6082289 775.2355942 832.0623746 993.5792311 1071.006761 2858.988102 4116.786658
  250.3684444 842.7200687 890.3703779 922.7245729 1022.379576 1064.789097 1305.561623 1354.935087
  2529.629134 2688.751974 3010.972581 3391.998462 3618.458565 3859.370573 3975.834707 4068.43033
  2490.530708 2527.368277 2593.973236 2642.677883 2686.154866 2749.900407 2807.215882 3079.586631
  1259.404703 1658.856064 2705.793176 2927.261303 2979.567182 3020.891247 3227.892415 4036.457914
  1434.666519 1485.729506 1674.469663 2560.596913 3721.993872 3792.434418 3929.077981 4014.804294
  452.6658301 1310.328999 1334.35291 1771.027439 1994.4533 3092.697406 4066.210621 7361.063351
  1695.956055 3067.67014 3646.731522 3939.813944 4196.690971 4322.223157 4756.994184 4903.200497
  564.3306616 668.0519514 2269.871795 2346.26633 3695.328218 4504.109535 4798.110836 4899.539141
  519.2485749 5051.469221 5339.158541 5533.120006 9808.506136 10962.47103 11147.03251 11354.48906
  821.2326731 3164.980797 3778.005625 3989.19205 4239.50251 4779.243735 5639.747312 7562.922068
  452.529775 817.123536 2743.400106 3033.869336 3988.80082 5886.281507 10564.73245 11225.92746
]))
(def voice_mode_rates (tensor @shape [184] @data [
  3.006411977 2.800586025 1.570869681 2.775111909 2.739754141 2.482719536 2.400045827 2.250059399
  0.5137395257 1.103550924 1.202586076 0.5310186451 1.212470664 1.053031609 1.044854761 0.7024868987
  2.240795452 2.42055731 1.724092522 1.473204107 2.73804592 2.877921688 3.380310558 3.596622619
  2.113565861 3.247883405 2.550552825 3.17674074 3.405525723 2.811064374 2.797531498 3.552501053
  1.408112764 1.545930842 1.706734668 1.933438375 1.593202334 2.792330917 2.337669575 2.654355407
  0.6732259097 0.3312240088 0.8644276318 0.4595758338 0.6732856297 0.6804329952 0.8365622172 1.339464804
  0.922319934 1.209176766 1.234172486 1.447730612 1.396264772 1.418934754 1.03656871 1.245888318
  0.9042860488 0.6548370271 1.047534775 0.9698513689 1.093316309 0.7893004706 1.108095881 1.785309437
  1.665445361 0.9820065009 0.2552420536 0.8055970728 1.96231708 1.259397956 0.721532479 1.621607141
  0.5175153987 1.484383845 1.899558868 0.8067842553 0.6362503671 1.22911868 0.8864503647 1.229567962
  1.271546619 0.7240973895 2.120339954 1.737495282 1.203943661 1.881885689 1.630268661 1.460094146
  2.799435289 2.25244959 2.782846067 2.710089464 2.741123857 2.126752526 2.928370727 2.620401367
  1.085243302 1.307298674 1.657045565 1.618991018 1.100518635 1.861563331 1.881982869 1.874292085
  1.467821007 1.886403985 2.074766365 2.329047489 2.20914704 2.359604579 2.082632243 2.515288707
  1.007067999 1.041386967 1.000918918 0.6623002 0.8154835624 0.981615503 0.7789339515 1.121211625
  1.199986418 0.7531755024 1.367219562 1.605333103 1.582852479 1.2695938 1.617988771 1.713947273
  2.419440879 1.31515998 1.522117257 1.815208844 1.665858694 2.702764244 2.990610041 2.187560074
  2.643392252 2.431142503 2.519952631 2.941439502 3.284542823 2.770999032 3.25991666 3.135874606
  1.657026782 1.667855187 1.302534875 1.478686989 1.980087603 2.174797703 1.85919036 1.879660976
  0.9633442523 0.8975858029 1.064511386 1.737247187 1.717034954 1.561407018 2.596818324 1.736267138
  1.273143567 1.742757117 2.118726615 1.8691642 1.542806703 2.416444336 1.926440048 2.016237292
  5.541190694 4.736636992 5.339087273 5.322001572 5.860287279 5.921111176 5.034074742 6.328161341
  2.956649676 5.76019195 5.035583427 4.172108376 5.077027341 6.200415906 6.22262984 6.108922623
]))
(def voice_mode_gains (tensor @shape [184] @data [
  0.01157484056 7.420563835e-13 7.327532145e-13 7.415324963e-13 0.04243503545 7.410579076e-13 7.385256789e-13 0.01270909004
  0.06216920725 0.09480435577 8.635066885e-15 0.0215045722 0.01526364858 0.0473247654 8.594210397e-15 0.02500352976
  0.005733354836 0.02883103374 0.03151983845 7.191325612e-13 0.01976069288 7.319826623e-13 7.338266829e-13 0.04078926161
  0.03401846665 0.04412604189 0.03527656706 7.31636061e-13 0.03842503069 0.01957386038 0.03228438032 0.03384061529
  0.06796419688 7.18971952e-13 0.1538704828 7.243710303e-13 7.19374171e-13 0.1152758558 0.1319789582 0.08417909353
  0.01417913906 0.007105432381 0.01508126299 0.009973831783 0.006315782283 5.670569768e-22 0.009900132365 0.01219632752
  0.02063506683 0.07325690221 0.07077722222 0.05060811306 0.01006011054 0.003387888638 1.30492453e-10 1.235309966e-10
  0.006432649763 0.01400354572 7.195411978e-13 0.005588577421 0.006840414346 0.005748264111 7.153082868e-13 0.01062157994
  0.06554801184 0.07041477467 0.03662091032 0.04278609222 0.1694751417 0.07357201408 0.04728795807 0.08801330659
  0.01983557497 0.02177385397 0.02534279041 0.01316997858 0.01253818135 0.01118519045 0.02136122856 0.01735207519
  0.05210781604 0.01267956505 7.397630968e-13 7.298383441e-13 0.04528273566 0.06478585177 0.04612941107 0.05152811408
  1.355351783e-10 0.01789121362 0.05820308191 0.04498882035 0.04302585937 0.03655514416 0.01651237924 0.03846709786
  0.01619930342 0.0254525535 0.01943142322 2.50636285e-11 0.009449320698 0.01558193093 0.02277914873 0.01853469654
  7.179313411e-13 7.354764186e-13 7.729782246e-13 0.0376277112 0.02434646938 0.04678715575 0.03536030854 0.04440107538
  0.02811528575 0.03485361183 1.427677512e-10 0.01138960777 1.424501637e-10 0.01914202903 0.01125733521 0.01437694384
  1.954069409e-13 1.945402462e-13 1.957560233e-13 1.962000655e-13 0.01058955679 0.009559349916 0.01209788592 0.0110111501
  5.805245415e-13 0.03699814159 5.726189012e-13 0.06598991985 0.04202542965 0.04129704196 0.03712188385 0.02991468081
  7.429781108e-13 0.0460655849 7.324285162e-13 7.590427618e-13 0.05762051238 0.03198005324 8.962339403e-11 0.05821694153
  0.03848426117 0.02193048326 0.01485962936 0.01882920265 0.02793579599 0.03314290567 0.02172878421 0.02309304296
  0.02431921048 6.689894531e-11 0.01576285802 0.01696912551 0.01353364781 0.0178056374 0.02467258004 0.02026589736
  0.02888018333 0.04586510632 0.03461618442 0.04706587065 0.03176435424 1.447359699e-10 0.04171236072 0.03990415321
  7.116389328e-13 7.072680579e-13 0.03262173868 0.02862708965 0.03436312289 0.03976191098 0.03688004646 7.175527145e-13
  0.01032075999 0.03129122691 0.01912831048 0.01979815789 0.02414536954 7.67363459e-13 0.05631948569 0.04288872661
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
(def voice_row (* character_v 22))
(def voice_lo (floor voice_row))
(def voice_hi (min 22 (+ voice_lo 1)))
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
